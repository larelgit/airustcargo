//! The render side of the action module on the real engine: a skinned glTF model, its clips in
//! a second file that shares the skeleton, characters driven by the action machine through
//! `Intent`, and the pose that comes out of Bevy's animation.
//!
//! The first test runs headless (the engine's render plugins stay registered without a GPU,
//! so glTF loading, skins and animation all work) and is part of the suite. The second needs
//! a display and a Vulkan driver; it is how the windowed side was looked at:
//!
//! ```text
//! xvfb-run -a cargo test -p shards --test action_render -- --ignored --nocapture
//! ```
//!
//! (software Vulkan: `mesa-vulkan-drivers`, lavapipe). It writes `shots/action_render.png`
//! into the assets folder's parent under `target/tmp`.

#![cfg(feature = "render")]

use std::path::{Path, PathBuf};
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

// --- a minimal skinned glTF ---------------------------------------------------------------

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A one-triangle mesh skinned to two bones, `Armature` > `Hips` > `Spine`, next to a `Body`
/// node that carries the mesh. With `animated`, two animations turn the spine about Z: `Idle`
/// by 10 degrees, `Slash` by 90, over one second.
fn skinned_gltf(animated: bool) -> String {
    let mut data: Vec<u8> = Vec::new();
    let mut push = |floats: &[f32]| floats.iter().for_each(|f| data.extend(f.to_le_bytes()));
    push(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // positions
    let mut joints = Vec::new();
    for _ in 0..3 {
        joints.extend([0u8, 1, 0, 0]);
    }
    drop(push);
    data.extend(joints); // joints
    let mut push = |floats: &[f32]| floats.iter().for_each(|f| data.extend(f.to_le_bytes()));
    for _ in 0..3 {
        push(&[0.5, 0.5, 0.0, 0.0]); // weights
    }
    drop(push);
    data.extend([0u16, 1, 2, 0].iter().flat_map(|i| i.to_le_bytes())); // indices (+ 2 bytes of padding)
    let mut push = |floats: &[f32]| floats.iter().for_each(|f| data.extend(f.to_le_bytes()));
    for lift in [-1.0f32, -1.5] {
        push(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, lift, 0.0, 1.0]); // inverse bind
    }
    push(&[0.0, 1.0]); // times
    let half = |degrees: f32| (degrees.to_radians() * 0.5).sin_cos();
    for degrees in [10.0, 90.0] {
        let (s, c) = half(degrees);
        push(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, c]);
    }
    assert_eq!(data.len(), 304);
    let mut doc = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0, 3]}],
        "nodes": [
            {"name": "Armature", "children": [1]},
            {"name": "Hips", "translation": [0.0, 1.0, 0.0], "children": [2]},
            {"name": "Spine", "translation": [0.0, 0.5, 0.0]},
            {"name": "Body", "mesh": 0, "skin": 0},
        ],
        "skins": [{"joints": [1, 2], "inverseBindMatrices": 4, "skeleton": 0}],
        "meshes": [{"name": "Body", "primitives": [{"attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2}, "indices": 3}]}],
        "buffers": [{"uri": format!("data:application/octet-stream;base64,{}", base64(&data)), "byteLength": 304}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 12},
            {"buffer": 0, "byteOffset": 48, "byteLength": 48},
            {"buffer": 0, "byteOffset": 96, "byteLength": 6},
            {"buffer": 0, "byteOffset": 104, "byteLength": 128},
            {"buffer": 0, "byteOffset": 232, "byteLength": 8},
            {"buffer": 0, "byteOffset": 240, "byteLength": 32},
            {"buffer": 0, "byteOffset": 272, "byteLength": 32},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5121, "count": 3, "type": "VEC4"},
            {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4"},
            {"bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR"},
            {"bufferView": 4, "componentType": 5126, "count": 2, "type": "MAT4"},
            {"bufferView": 5, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0.0], "max": [1.0]},
            {"bufferView": 6, "componentType": 5126, "count": 2, "type": "VEC4"},
            {"bufferView": 7, "componentType": 5126, "count": 2, "type": "VEC4"},
        ],
    });
    if animated {
        let clip = |name: &str, output: u32| {
            json!({
                "name": name,
                "samplers": [{"input": 5, "output": output, "interpolation": "LINEAR"}],
                "channels": [{"sampler": 0, "target": {"node": 2, "path": "rotation"}}],
            })
        };
        doc["animations"] = json!([clip("Idle", 6), clip("Slash", 7)]);
    }
    doc.to_string()
}

const ACTIONS: &str = r#"{
    "light_1": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20)]),
    "wind_up": (net_id: 2, phases: [("windup", 0.8), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Circle(radius: 3.0), damage: 20)],
        telegraph: (symbol: "!", sound: "growl")),
}"#;
const MOVESETS: &str = r#"{ "hero": (inputs: {"light": "light_1"}), "brute": (inputs: {"light": "wind_up"}) }"#;
const ANIMSETS: &str = r#"{ "humanoid": (
    source: "models/humanoid.gltf",
    clips: {
        "idle": (name: "Idle", cues: [(at: 0.5, cue: "breath")]),
        "slash": (name: "Slash", cues: [(at: 0.5, cue: "swoosh")]),
    },
    locomotion: {"idle": "idle"},
    actions: {"light_1": "slash"},
) }"#;

/// A fresh assets folder under the test temp dir: content, the model (no animations) and the
/// library (the same skeleton with its clips).
fn assets(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        ("content/combat/actions.action.ron", ACTIONS.to_owned()),
        ("content/combat/movesets.moveset.ron", MOVESETS.to_owned()),
        ("content/anim/humanoid.animset.ron", ANIMSETS.to_owned()),
        ("models/model.gltf", skinned_gltf(false)),
        ("models/humanoid.gltf", skinned_gltf(true)),
    ] {
        let path: &Path = path.as_ref();
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), text).unwrap();
    }
    dir
}

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
    let (a, b) = (app.world().get::<AnimationGraphHandle>(player).unwrap(), app.world().get::<AnimationGraphHandle>(other).unwrap());
    assert_eq!(a.id(), b.id(), "one graph for both characters: the clips are shared");

    // Standing: the idle clip, looping on its own (its first cue passes as time goes by).
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

