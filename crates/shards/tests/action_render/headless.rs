//! A model follows the action machine on the headless engine.

use std::time::{Duration, Instant};

use shards::action::render::{AnimGraphs, AnimationBound, GltfModel};
use shards::action::{ActionState, Animated, AnimationCue};
use shards::bevy::animation::graph::AnimationGraphHandle;
use shards::bevy::animation::graph::AnimationNodeIndex;
use shards::bevy::animation::{AnimationClip, AnimationPlayer, AnimationTargetId};
use shards::bevy::gltf::Gltf;
use shards::bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};
use shards::motion::{character, Collider, Shape};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::ok;

use crate::fixtures::{assets, humanoid_assets, humanoid_assets_with, HUMANOID_LAYERED};

#[derive(Resource, Default)]
struct Cues(Vec<AnimationCue>);

fn spawn_hero(app: &mut App, at: Vec3) -> Entity {
    spawn_model(app, at, "models/model.gltf")
}

/// The `Gltf` files the tests loaded whole, kept alive for the app's life.
#[derive(Resource, Default)]
struct Loaded(Vec<Handle<Gltf>>);

/// A hero with the model of `file`. The file is loaded whole first and the model is its scene:
/// a load of `file#Scene0` always reads the file again (bevy#10549), and when the bridge's load of
/// the same file ends after the model is out, the model is spawned again under the test.
pub(crate) fn spawn_model(app: &mut App, at: Vec3, file: &'static str) -> Entity {
    let gltf: Handle<Gltf> = app.world().resource::<AssetServer>().load(file);
    update_until(app, &format!("{file} loaded"), |app| {
        app.world().resource::<AssetServer>().is_loaded_with_dependencies(&gltf)
    });
    let model = app.world().resource::<Assets<Gltf>>().get(&gltf).expect("the file is loaded").scenes[0].clone();
    app.world_mut().get_resource_or_init::<Loaded>().0.push(gltf);
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
pub(crate) fn update_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        assert!(started.elapsed() < Duration::from_secs(30), "gave up waiting for: {what}");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The angle (degrees) the spine bone of `character` is turned about Z.
fn spine_angle(app: &mut App, character: Entity) -> f32 {
    bone(app, character, "Spine").rotation.to_euler(EulerRot::ZYX).0.to_degrees()
}

/// The local transform of bone `name` of `character`'s model.
pub(crate) fn bone(app: &mut App, character: Entity, name: &str) -> Transform {
    let world = app.world_mut();
    let mut names = world.query::<(Entity, &Name)>();
    let bone = names
        .iter(world)
        .find(|(e, n)| n.as_str() == name && is_below(world, *e, character))
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("the model has no {name}"));
    *world.get::<Transform>(bone).unwrap()
}

pub(crate) fn is_below(world: &World, mut entity: Entity, ancestor: Entity) -> bool {
    while let Some(parent) = world.get::<ChildOf>(entity) {
        entity = parent.parent();
        if entity == ancestor {
            return true;
        }
    }
    false
}

pub(crate) fn progress(app: &App, character: Entity) -> Option<f32> {
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

#[test]
fn the_blender_humanoid_rolls_in_place_and_bows_with_a_clip_of_its_second_file() {
    let dir = humanoid_assets("action_render_humanoid");
    let mut app = shards::testing::test_app(|config| config.asset_dir = dir.to_string_lossy().into_owned(), |_| {});
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "the set's files have its clips and its root bone: {errors:#?}");
    app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::new(30.0, 0.5, 30.0))), Transform::from_xyz(0.0, -0.5, 0.0)));
    let hero = spawn_model(&mut app, Vec3::ZERO, "models/humanoid.glb");
    update_until(&mut app, "the humanoid bound", |app| app.world().get::<AnimationBound>(hero).is_some());
    let built = app.world().resource::<AnimGraphs>().built("humanoid").expect("the set's graph");
    assert_eq!(built.clips(), ["bow", "idle", "light_1", "roll"], "the bow comes from humanoid_gestures.glb");
    let player = app.world().get::<AnimationBound>(hero).unwrap().player;
    assert_eq!(app.world().get::<Name>(player).map(|n| n.to_string()).as_deref(), Some("Humanoid"));
    update_until(&mut app, "idle", |app| app.world().get::<AnimationBound>(hero).unwrap().clip() == Some("idle"));
    app.update();
    let (hips, spine) = (bone(&mut app, hero, "hips").rotation, bone(&mut app, hero, "spine").rotation);

    // Halfway through the roll the hips are upside down, and the root bone, which the clip moves
    // metres forward, stays where it rests: the body itself moves by the action's displacement.
    let start = app.world().get::<Transform>(hero).unwrap().translation;
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("dodge");
    ok(&mut app, "step", json!({"ticks": 12}));
    app.update();
    app.update();
    assert_eq!(app.world().get::<AnimationBound>(hero).unwrap().clip(), Some("roll"));
    let turned = bone(&mut app, hero, "hips").rotation.angle_between(hips).to_degrees();
    assert!(turned > 60.0, "the hips turn over: {turned:.0} degrees");
    let root = bone(&mut app, hero, "root").translation;
    assert!(root.x.abs() < 1e-4 && root.z.abs() < 1e-4, "the root stays in place along the ground: {root}");
    let moved = app.world().get::<Transform>(hero).unwrap().translation.distance(start);
    assert!(moved > 1.0, "the body rolls by the action's displacement: {moved:.2} m");

    // The block shows the bow, a clip of the second file, on the model of the first.
    ok(&mut app, "step", json!({"ticks": 30}));
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("block");
    ok(&mut app, "step", json!({"ticks": 16}));
    app.update();
    app.update();
    assert_eq!(app.world().get::<AnimationBound>(hero).unwrap().clip(), Some("bow"));
    let bent = bone(&mut app, hero, "spine").rotation.angle_between(spine).to_degrees();
    assert!(bent > 20.0, "the spine bends in the bow: {bent:.0} degrees");
}

/// The entity of bone `name` of `character`'s model.
pub(crate) fn bone_entity(app: &mut App, character: Entity, name: &str) -> Entity {
    let world = app.world_mut();
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find(|(e, n)| n.as_str() == name && is_below(world, *e, character))
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("the model has no {name}"))
}

/// The clip `name` of the loaded glTF files (the humanoid's, or its second file's).
fn library_clip(app: &App, name: &str) -> Handle<AnimationClip> {
    let gltfs = app.world().resource::<Assets<Gltf>>();
    gltfs.iter().find_map(|(_, gltf)| gltf.named_animations.get(name).cloned()).unwrap_or_else(|| panic!("no clip {name}"))
}

/// The rotation clip `clip` gives `bone` at `seconds`, sampled straight from its curves.
fn sampled(app: &App, clip: &Handle<AnimationClip>, bone: Entity, seconds: f32) -> Quat {
    let target = *app.world().get::<AnimationTargetId>(bone).expect("a bone is an animation target");
    let clip = app.world().resource::<Assets<AnimationClip>>().get(clip).expect("the clip is loaded");
    let curves = clip.curves_for_target(target).expect("the clip animates the bone");
    curves
        .iter()
        .find_map(|curve| curve.0.sample_clamped(seconds).downcast::<Quat>().ok())
        .map(|rotation| *rotation)
        .expect("a rotation curve")
}

/// How far (degrees) bone `name` of `character` is from what `clip` gives it at `seconds`.
fn off_by(app: &mut App, character: Entity, name: &str, clip: &Handle<AnimationClip>, seconds: f32) -> f32 {
    let bone = bone_entity(app, character, name);
    let rotation = app.world().get::<Transform>(bone).unwrap().rotation;
    rotation.angle_between(sampled(app, clip, bone, seconds)).to_degrees()
}

/// Seconds on the clock of the clip at graph node `node` of `character`'s player.
fn seek_time(app: &App, character: Entity, node: AnimationNodeIndex) -> f32 {
    let player = app.world().get::<AnimationBound>(character).unwrap().player;
    app.world().get::<AnimationPlayer>(player).unwrap().animation(node).expect("the clip is playing").seek_time()
}

const LEGS: [&str; 7] = ["hips", "thigh.L", "shin.L", "foot.L", "thigh.R", "shin.R", "foot.R"];
const UPPER: [&str; 8] = ["spine", "head", "upper_arm.L", "forearm.L", "hand.L", "upper_arm.R", "forearm.R", "hand.R"];

#[test]
fn the_blender_humanoid_drinks_with_its_arm_while_its_legs_keep_walking() {
    let dir = humanoid_assets_with("action_render_layers", HUMANOID_LAYERED);
    let mut app = shards::testing::test_app(|config| config.asset_dir = dir.to_string_lossy().into_owned(), |_| {});
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "the set's files have its clips and the layer's bone: {errors:#?}");
    app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::new(30.0, 0.5, 30.0))), Transform::from_xyz(0.0, -0.5, 0.0)));
    let hero = spawn_model(&mut app, Vec3::ZERO, "models/humanoid.glb");
    update_until(&mut app, "the humanoid bound", |app| {
        app.world().get::<AnimationBound>(hero).is_some_and(|b| b.layer_clip("upper").is_none())
            && app.world().resource::<AnimGraphs>().built("humanoid").is_some()
    });
    let (walk_node, drink_node) = {
        let built = app.world().resource::<AnimGraphs>().built("humanoid").unwrap();
        assert_eq!(built.clips(), ["drink", "idle", "walk"]);
        (built.node("walk").unwrap(), built.layer_node("upper", "drink").expect("the drink has a node for its layer"))
    };
    let (walk, drink) = (library_clip(&app, "walk"), library_clip(&app, "drink"));

    // Walking: every bone follows the walk clip.
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = Vec3::new(0.0, 0.0, 0.5);
    ok(&mut app, "step", json!({"ticks": 20}));
    app.update();
    app.update();
    assert_eq!(app.world().get::<AnimationBound>(hero).unwrap().clip(), Some("walk"));
    let at = seek_time(&app, hero, walk_node);
    for bone in LEGS.into_iter().chain(UPPER) {
        let off = off_by(&mut app, hero, bone, &walk, at);
        assert!(off < 0.1, "{bone} follows the walk: {off:.3} degrees off");
    }

    // The flask: the upper body drinks, the legs keep their walk.
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("drink");
    ok(&mut app, "step", json!({"ticks": 30}));
    app.update();
    app.update();
    let p = progress(&app, hero).expect("the drink is running");
    assert!(p > 0.2 && p < 0.6, "{p}");
    let bound = app.world().get::<AnimationBound>(hero).unwrap();
    assert_eq!(bound.clip(), Some("walk"), "locomotion goes on");
    assert_eq!(bound.layer_clip("upper"), Some("drink"));
    let length = app.world().resource::<Assets<AnimationClip>>().get(&drink).unwrap().duration();
    let (drink_at, walk_at) = (seek_time(&app, hero, drink_node), seek_time(&app, hero, walk_node));
    assert!((drink_at / length - p).abs() < 0.02, "gameplay leads: the clip is at {:.3}, the action at {p:.3}", drink_at / length);
    for bone in LEGS {
        let off = off_by(&mut app, hero, bone, &walk, walk_at);
        assert!(off < 0.1, "{bone} still walks: {off:.3} degrees off the walk clip");
    }
    for bone in UPPER {
        let off = off_by(&mut app, hero, bone, &drink, drink_at);
        assert!(off < 0.1, "{bone} drinks: {off:.3} degrees off the drink clip");
    }
    // And the two clips are not the same pose, so the checks above tell them apart.
    let (arm, thigh) = (bone_entity(&mut app, hero, "upper_arm.R"), bone_entity(&mut app, hero, "thigh.L"));
    let (arm_walks, thigh_drinks) = (sampled(&app, &walk, arm, drink_at), sampled(&app, &drink, thigh, walk_at));
    let drinking_arm = app.world().get::<Transform>(arm).unwrap().rotation;
    let walking_thigh = app.world().get::<Transform>(thigh).unwrap().rotation;
    assert!(drinking_arm.angle_between(arm_walks).to_degrees() > 30.0, "the walk swings the arm, the drink raises it");
    assert!(walking_thigh.angle_between(thigh_drinks).to_degrees() > 10.0, "the walk swings the leg, the drink stands");

    // The drink ends: the arms are the walk's again.
    ok(&mut app, "step", json!({"ticks": 60}));
    app.update();
    app.update();
    assert!(progress(&app, hero).is_none());
    let bound = app.world().get::<AnimationBound>(hero).unwrap();
    assert_eq!(bound.layer_clip("upper"), None);
    assert_eq!(bound.layer_share("upper"), 0.0);
    let at = seek_time(&app, hero, walk_node);
    for bone in LEGS.into_iter().chain(UPPER) {
        let off = off_by(&mut app, hero, bone, &walk, at);
        assert!(off < 0.1, "{bone} walks again: {off:.3} degrees off");
    }
}

/// The models `WorldInstanceReady` was triggered on, in order.
#[derive(Resource, Default)]
struct Spawned(Vec<Entity>);

/// The engine's `GltfModel` (the sandbox's and the template's way): the model's file is loaded whole,
/// the same handle the bridge loads for its clips (the humanoid's clips are in its own file), and the
/// model is spawned once when the file is in. A `file#Scene0` model could be spawned twice: its
/// labeled load reads the file again (bevy#10549) and may end after the bridge's.
#[test]
fn a_gltf_model_shares_the_bridges_load_and_is_spawned_once() {
    let dir = humanoid_assets("action_render_gltf_model");
    let mut app = shards::testing::test_app(
        |config| config.asset_dir = dir.to_string_lossy().into_owned(),
        |app| {
            app.init_resource::<Spawned>();
            app.add_observer(|ready: On<WorldInstanceReady>, mut spawned: ResMut<Spawned>| spawned.0.push(ready.entity));
        },
    );
    let model = GltfModel::load(app.world().resource::<AssetServer>(), "models/humanoid.glb");
    let file = model.file.id();
    let (controller, intent, transform) = character("", Vec3::ZERO);
    let world = app.world_mut();
    let hero = world
        .spawn((Name::new("hero"), Fighter::new("hero"), Health::new(100.0), controller, intent, transform, Animated::new("humanoid")))
        .id();
    let shown = world.spawn((model, ChildOf(hero))).id();
    update_until(&mut app, "the hero bound", |app| app.world().get::<AnimationBound>(hero).is_some());
    // Time for a second read to land, had there been one.
    for _ in 0..40 {
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.world().get::<WorldAssetRoot>(shown).is_some(), "the scene went on the model's entity");
    let spawned = &app.world().resource::<Spawned>().0;
    assert_eq!(spawned.iter().filter(|e| **e == shown).count(), 1, "spawned once: {spawned:?}");
    let bridge = app.world().resource::<AssetServer>().get_handle::<Gltf>("models/humanoid.glb").expect("loaded");
    assert_eq!(bridge.id(), file, "one handle for the model and the bridge's clips");
    // A scene the file does not have spawns nothing.
    let other = GltfModel::load(app.world().resource::<AssetServer>(), "models/humanoid.glb").scene(9);
    let none = app.world_mut().spawn(other).id();
    app.update();
    assert!(app.world().get::<WorldAssetRoot>(none).is_none());

    // The model changed to another file: its scene is replaced once that file is in, and the old
    // scene's entities go.
    let before: Vec<Entity> = app.world().get::<Children>(shown).map(|c| c.iter().collect()).unwrap_or_default();
    assert!(!before.is_empty(), "the humanoid's scene is under the model");
    let gestures = GltfModel::load(app.world().resource::<AssetServer>(), "models/humanoid_gestures.glb");
    let file = gestures.file.clone();
    app.world_mut().entity_mut(shown).insert(gestures);
    update_until(&mut app, "the second file's scene", |app| {
        let gltf = app.world().resource::<Assets<Gltf>>().get(&file);
        let root = app.world().get::<WorldAssetRoot>(shown);
        matches!((gltf, root), (Some(gltf), Some(root)) if root.0 == gltf.scenes[0])
    });
    for _ in 0..5 {
        app.update();
    }
    assert!(before.iter().all(|e| app.world().get_entity(*e).is_err()), "the humanoid's scene is gone");
    let spawned = &app.world().resource::<Spawned>().0;
    assert_eq!(spawned.iter().filter(|e| **e == shown).count(), 2, "spawned once more, for the new file: {spawned:?}");

    // Changed to a scene the file does not have: the scene shown so far goes, and nothing takes its place.
    let before: Vec<Entity> = app.world().get::<Children>(shown).map(|c| c.iter().collect()).unwrap_or_default();
    assert!(!before.is_empty(), "the gestures' scene is under the model");
    let missing = GltfModel::new(file).scene(9);
    app.world_mut().entity_mut(shown).insert(missing);
    for _ in 0..5 {
        app.update();
    }
    assert!(app.world().get::<WorldAssetRoot>(shown).is_none(), "no root for a scene that is not there");
    assert!(before.iter().all(|e| app.world().get_entity(*e).is_err()), "the gestures' scene is gone");
    assert!(app.world().get_entity(shown).is_ok(), "the model's own entity stays");
}
