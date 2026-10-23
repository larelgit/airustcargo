//! Lights from content in the sandbox (F13): the lit room `lit_room` (`assets/levels/lit_room.gltf`, written by
//! [`room`]; its lights in `content/light/lights.light.ron`, its grading in `grading.grading.ron`), the level
//! properties `light=` and `grading=`, the `lights` query, and (with a GPU, `--ignored`) the pictures: a flashing lamp
//! lights the room at one moment and not at another, and the grading volume changes the picture's colours.
//! Regenerate the room's file after changing [`room`]:
//!
//! ```text
//! SHARDS_UPDATE_FIXTURES=1 cargo test -p sandbox --test sandbox lights::the_committed_room
//! ```
//!
//! `scenarios/lights.scenario` plays the room with queries alone.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shards::ai_core::display::Offscreen;
use shards::bevy::light::{DirectionalLight, GlobalAmbientLight, PointLight, SpotLight};
use shards::bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use shards::content::ContentDb;
use shards::level::{check_level, spawn_level_by_id, LevelAsset};
use shards::light::{ContentLight, LightPicture, LightState};
use shards::motion::camera::{GradingBlend, GradingVolume};
use shards::motion::{Collider, Volume, VolumeKind};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::*;
use crate::gltf::{Gltf, NodeSpec};

/// The room, in its own frame (the level `lit_room` places it on the canyon floor between the course's two blocks of rock, at x 112): a floor 12 x 12 m with a back wall and two
/// side walls 4 m high and no ceiling or front; a pillar; six lights, four of them on Blender lamps (`Lamp.Steady`,
/// `Lamp.Flash`, `Lamp.Sodium` and the spot `Spot.Torch`, each with a lamp of its own in the file that the
/// content light replaces) and two on empties (`Lamp.Tube`, `Lamp.Dim`); and a trigger volume over the right half
/// (x from 0 to 6) that grades the picture.
pub fn room() -> Gltf {
    let mut g = Gltf::new();
    let floor = g.box_mesh("Floor", [-6.0, -0.2, -6.0], [6.0, 0.0, 6.0]);
    let back = g.box_mesh("Back", [-6.0, 0.0, -6.3], [6.0, 4.0, -6.0]);
    let left = g.box_mesh("Left", [-6.3, 0.0, -6.0], [-6.0, 4.0, 6.0]);
    let right = g.box_mesh("Right", [6.0, 0.0, -6.0], [6.3, 4.0, 6.0]);
    let pillar = g.box_mesh("Pillar", [-0.5, 0.0, -0.5], [0.5, 3.0, 0.5]);
    g.root(NodeSpec::new("Floor").mesh(floor).props(json!({"collider": "auto"})));
    g.root(NodeSpec::new("Back").mesh(back));
    g.root(NodeSpec::new("Left").mesh(left));
    g.root(NodeSpec::new("Right").mesh(right));
    g.root(NodeSpec::new("Pillar").at([1.5, 0.0, -2.0]).mesh(pillar).props(json!({"collider": "auto"})));
    g.root(NodeSpec::new("Door").at([0.0, 0.0, 6.0]).props(json!({"marker": "spawn", "name": "door"})));
    g.root(NodeSpec::new("Lamp.Steady").at([-3.0, 3.0, 0.0]).props(json!({"light": "lamp"})));
    g.root(NodeSpec::new("Lamp.Flash").at([0.0, 3.0, -4.0]).props(json!({"light": "flasher"})));
    g.root(NodeSpec::new("Lamp.Sodium").at([-5.0, 3.0, -2.0]).props(json!({"light": "sodium"})));
    g.root(NodeSpec::new("Lamp.Tube").at([3.0, 3.5, -4.0]).props(json!({"light": "tube"})));
    g.root(NodeSpec::new("Lamp.Dim").at([5.0, 3.0, 2.0]).props(json!({"light": "dimmer"})));
    // The torch points (along its -Z) at the pillar's foot.
    let mut torch = NodeSpec::new("Spot.Torch").at([4.0, 3.5, 3.0]).props(json!({"light": "torch"}));
    torch.rotation = Some(Quat::from_rotation_arc(Vec3::NEG_Z, (Vec3::new(1.5, 0.5, -2.0) - Vec3::new(4.0, 3.5, 3.0)).normalize()).to_array());
    g.root(torch);
    g.root(NodeSpec::new("Zone.Rust").at([3.0, 2.0, 0.0]).props(json!({"marker": "trigger", "name": "rust_zone", "grading": "rust", "collider": "box", "size": [6.0, 4.0, 12.0]})));
    g
}

/// The file's text: [`room`] as the writer makes it, with the lamps Blender's exporter writes with *Punctual Lights*
/// (`KHR_lights_punctual`) on the four lamp nodes.
pub fn room_file() -> String {
    let mut doc: Value = shards::serde_json::from_str(&room().gltf()).unwrap();
    let lamps = [
        ("Lamp.Steady", json!({"type": "point", "name": "Steady", "color": [1.0, 0.9, 0.7], "intensity": 300.0})),
        ("Lamp.Flash", json!({"type": "point", "name": "Flash", "color": [1.0, 1.0, 1.0], "intensity": 900.0})),
        ("Lamp.Sodium", json!({"type": "point", "name": "Sodium", "color": [1.0, 0.7, 0.3], "intensity": 300.0})),
        ("Spot.Torch", json!({"type": "spot", "name": "Torch", "color": [0.9, 0.95, 1.0], "intensity": 600.0, "spot": {"innerConeAngle": 0.2, "outerConeAngle": 0.4}})),
    ];
    let mut defs = Vec::new();
    for (node, light) in lamps {
        let at = doc["nodes"].as_array().unwrap().iter().position(|n| n["name"] == node).unwrap_or_else(|| panic!("no node {node}"));
        doc["nodes"][at]["extensions"] = json!({"KHR_lights_punctual": {"light": defs.len()}});
        defs.push(light);
    }
    doc["extensionsUsed"] = json!(["KHR_lights_punctual"]);
    doc["extensions"] = json!({"KHR_lights_punctual": {"lights": defs}});
    let mut text = shards::serde_json::to_string_pretty(&doc).unwrap();
    text.push('\n');
    text
}

fn room_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/levels/lit_room.gltf")
}

#[test]
fn the_committed_room_is_what_the_builder_writes() {
    let want = room_file();
    if std::env::var_os("SHARDS_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(room_path().parent().unwrap()).unwrap();
        std::fs::write(room_path(), &want).unwrap();
    }
    let have = std::fs::read_to_string(room_path()).unwrap_or_else(|e| panic!("{}: {e}; run with SHARDS_UPDATE_FIXTURES=1", room_path().display()));
    assert!(have == want, "assets/levels/lit_room.gltf is out of date; regenerate it (see the module docs)");
}

/// Where the level is laid, and a point in its frame as the world sees it.
const AT: Vec3 = Vec3::new(112.0, -10.0, 0.0);

fn world_of(local: Vec3) -> Vec3 {
    AT + local
}

fn app_with_room() -> App {
    let mut app = sandbox::test_app(42);
    // The arena's fighters stand down, as in the scenarios: a test of the lights fights nobody.
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    spawn_level_by_id(app.world_mut(), "lit_room", Some(Transform::from_translation(AT))).unwrap();
    app
}

/// The player stands at `local` (the room's frame, feet 0.3 m above the floor).
fn stand_at(app: &mut App, local: Vec3) {
    let at = world_of(local);
    ok(app, "teleport", json!({"id": PLAYER, "x": at.x, "y": at.y + 0.3, "z": at.z}));
}

fn lights(app: &mut App, id: Option<&str>) -> String {
    let args = id.map_or_else(|| json!({}), |id| json!({"id": id}));
    text(&ok(app, "lights", args)).to_owned()
}

#[test]
fn the_level_places_six_content_lights_and_a_grading_volume_and_nothing_with_an_id() {
    let mut app = sandbox::test_app(42);
    let spawned = spawn_level_by_id(app.world_mut(), "lit_room", Some(Transform::from_translation(AT))).unwrap();
    // The floor and the pillar (solids), the door, six lights and the zone.
    assert_eq!(spawned.entities.len(), 10);
    let world = app.world_mut();
    let mut found = world.query::<(Entity, &Name, &ContentLight, &Transform)>();
    let mut placed: Vec<(String, String, Vec3, bool)> =
        found.iter(world).map(|(e, name, light, at)| (name.to_string(), light.id.clone(), at.translation, world.get::<AiId>(e).is_some())).collect();
    placed.sort_by(|a, b| a.0.cmp(&b.0));
    let names: Vec<(&str, &str)> = placed.iter().map(|(n, i, ..)| (n.as_str(), i.as_str())).collect();
    assert_eq!(
        names,
        [("Lamp.Dim", "dimmer"), ("Lamp.Flash", "flasher"), ("Lamp.Sodium", "sodium"), ("Lamp.Steady", "lamp"), ("Lamp.Tube", "tube"), ("Spot.Torch", "torch")]
    );
    assert_eq!(placed[3].2, world_of(Vec3::new(-3.0, 3.0, 0.0)), "a light stands where its node does");
    assert!(placed.iter().all(|p| !p.3), "a light is the picture's: no AiId, so no replay or snapshot sees it");

    let mut zone = world.query::<(&GradingVolume, &Volume, &Collider, &Transform)>();
    let zones: Vec<_> = zone.iter(world).map(|(g, v, _, at)| (g.id.clone(), v.kind.clone(), at.translation)).collect();
    assert_eq!(zones, [("rust".to_owned(), VolumeKind::Trigger { name: "rust_zone".into() }, world_of(Vec3::new(3.0, 2.0, 0.0)))]);
}

#[test]
fn the_lights_query_reads_each_lamp_off_the_simulations_clock() {
    let mut app = app_with_room();
    stand_at(&mut app, Vec3::new(-3.0, 0.0, 1.0));
    step(&mut app, 30);
    let said = lights(&mut app, None);
    assert!(said.starts_with("lights: 6 (point 5, spot 1) · 4 flicker · 3 want a shadow, 3 cast one (budget 4: graphics.shadow_lights) · clock 0.500 s"), "{said}");
    // Half a second in: the flasher's first letter (`m`, 100 %), and the one-letter pattern's fixed half.
    assert!(lights(&mut app, Some("flasher")).contains("pattern `ma` at 1/s, step 0 = m: 100 % (400000 lm)"));
    assert!(lights(&mut app, Some("dimmer")).contains("step 0 = g: 50 % (50000 lm)"));
    // At 1.5 s it is the second letter, `a`: dark. At 2.5 s the pattern has looped.
    step(&mut app, 60);
    assert!(lights(&mut app, Some("flasher")).contains("step 1 = a: 0 % (0 lm)"));
    assert!(lights(&mut app, Some("dimmer")).contains("50 % (50000 lm)"));
    step(&mut app, 60);
    assert!(lights(&mut app, Some("flasher")).contains("step 0 = m: 100 %"));
    // The picture's state is the same thing the status shows.
    let state = shards::ai_core::status::status(app.world())["light"].clone();
    assert_eq!((state["lights"].as_u64(), state["casting"].as_u64(), state["flickering"].as_u64()), (Some(6), Some(3), Some(4)), "{state}");
}

#[test]
fn a_light_prefab_places_a_content_light_too() {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "spawn_prefab", json!({"id": "lit_lamp", "x": 5.0, "y": 3.0, "z": 5.0}));
    step(&mut app, 1);
    let said = lights(&mut app, None);
    assert!(said.contains("lights: 1 (point 1, spot 0)"), "{said}");
    assert!(said.contains("lamp \"Lamp\" point at (5.0, 3.0, 5.0)"), "{said}");
    // The prefab gave it an id, so `despawn` takes it away again.
    let lamp = text(&ok(&mut app, "observe", json!({}))).lines().find(|l| l.contains("Lamp")).unwrap().split_whitespace().next().unwrap().to_owned();
    ok(&mut app, "despawn", json!({"id": lamp}));
    step(&mut app, 1);
    assert!(lights(&mut app, None).contains("lights: 0 (point 0, spot 0)"));
}

#[test]
fn bad_light_and_grading_properties_are_named_by_node() {
    let mut g = Gltf::new();
    g.root(NodeSpec::new("Lamp.Ghost").props(json!({"light": "nothing"})));
    g.root(NodeSpec::new("Lamp.Typo").props(json!({"light": "sodum"})));
    g.root(NodeSpec::new("Lamp.Empty").props(json!({"light": ""})));
    g.root(NodeSpec::new("Lamp.Number").props(json!({"light": 3})));
    g.root(NodeSpec::new("Lamp.Misspelt").props(json!({"ligth": "lamp"})));
    g.root(NodeSpec::new("Zone.Loose").props(json!({"grading": "rust"})));
    g.root(NodeSpec::new("Zone.Water").props(json!({"volume": "water", "collider": "box", "size": [2.0, 2.0, 2.0], "grading": "rust"})));
    g.root(NodeSpec::new("Zone.Ghost").props(json!({"marker": "trigger", "name": "z", "collider": "box", "size": [2.0, 2.0, 2.0], "grading": "rus"})));
    g.root(NodeSpec::new("Zone.Good").props(json!({"marker": "trigger", "name": "g", "collider": "box", "size": [2.0, 2.0, 2.0], "grading": "cold"})));
    g.root(NodeSpec::new("Lamp.Good").props(json!({"light": "torch"})));
    let level = LevelAsset::from_slice("bad.gltf", g.gltf().as_bytes()).unwrap();
    let app = sandbox::test_app(42);
    let registry = app.world().resource::<AppTypeRegistry>().read();
    let problems: Vec<String> = check_level(&level, &registry, Some(app.world().resource::<ContentDb>()), &[]).iter().map(ToString::to_string).collect();
    let all = problems.join("\n");
    for wanted in [
        "node `Lamp.Ghost`: `light`: no light definition `nothing`; lights: dimmer, flasher, lamp, sodium, torch, tube",
        "node `Lamp.Typo`: `light`: no light definition `sodum` (did you mean `sodium`?)",
        "node `Lamp.Empty`: `light` is empty; name a light definition",
        "node `Lamp.Number`: `light` must be text, found a number",
        "node `Lamp.Misspelt`: unknown property `ligth` (did you mean `light`?)",
        "node `Zone.Loose`: `grading` goes on a trigger volume: add `marker` = `trigger`",
        "node `Zone.Water`: `grading` goes on a trigger volume",
        "node `Zone.Ghost`: `grading`: no grading definition `rus` (did you mean `rust`?); gradings: cold, rust",
    ] {
        assert!(all.contains(wanted), "`{wanted}` in\n{all}");
    }
    assert!(!all.contains("Lamp.Good") && !all.contains("Zone.Good"), "{all}");
    assert_eq!(problems.len(), 8, "{all}");
}

#[test]
fn the_grading_volume_blends_the_picture_while_the_player_is_inside() {
    let mut app = app_with_room();
    let blend = |app: &App| app.world().resource::<GradingBlend>().clone();
    // On the left half, outside the volume: the camera's own picture.
    stand_at(&mut app, Vec3::new(-2.0, 0.0, 1.0));
    step(&mut app, 5);
    assert_eq!(blend(&app).volume(), None);
    let clock = |app: &App| shards::motion::camera::picture_clock(app.world().resource::<Time<Fixed>>());
    // Across the line: the first tick sees the volume; the blend is 1.5 s long and eased.
    stand_at(&mut app, Vec3::new(2.0, 0.0, 1.0));
    step(&mut app, 3);
    let touching = app.world().get::<MotionState>(app.world().resource::<shards::ai_core::AiIndex>().get(AiId::parse(PLAYER).unwrap()).unwrap()).unwrap().volumes.len();
    assert_eq!(touching, 1, "the player's capsule touches the zone");
    assert_eq!(blend(&app).volume(), Some("rust"));
    step(&mut app, 42);
    let (b, now) = (blend(&app), clock(&app));
    assert!((b.progress(now) - 44.0 / 90.0).abs() < 0.02, "{}", b.progress(now));
    let shown = b.current(now, None, &|id| app.world().resource::<ContentDb>().peek::<shards::motion::camera::GradingDef>(id)).unwrap();
    assert!(shown.temperature > 0.0 && shown.temperature < 0.1, "on its way: {shown:?}");
    step(&mut app, 60);
    let (b, now) = (blend(&app), clock(&app));
    assert!(!b.is_blending(now));
    let held = b.current(now, None, &|id| app.world().resource::<ContentDb>().peek::<shards::motion::camera::GradingDef>(id)).unwrap();
    assert_eq!((held.temperature, held.saturation), (0.1, 0.8));
    // And back out.
    stand_at(&mut app, Vec3::new(-2.0, 0.0, 1.0));
    step(&mut app, 1);
    assert_eq!(blend(&app).volume(), None);
    step(&mut app, 100);
    let (b, now) = (blend(&app), clock(&app));
    assert!(!b.is_blending(now) && b.current(now, None, &|_| None).is_none(), "the camera's own again");
    // The room's six lights are untouched by it all (a frame refreshes the state).
    app.update();
    assert_eq!(app.world().resource::<LightState>().lights.len(), 6);
}

// --- the pictures (a GPU) --------------------------------------------------------------------

fn offscreen_app() -> App {
    shards::testing::test_app(
        |config| (config.headless, config.offscreen) = (false, true),
        |app| {
            app.add_plugins((sandbox::SandboxPlugin, sandbox::visuals::VisualsPlugin));
        },
    )
}

fn settle(app: &mut App, frames: u32) {
    for _ in 0..frames {
        app.update();
    }
}

/// `image` as the GPU drew it, read back.
fn capture(app: &mut App, image: Handle<Image>) -> Image {
    let slot: Arc<Mutex<Option<Image>>> = Arc::default();
    let into = slot.clone();
    app.world_mut().spawn(Screenshot::image(image)).observe(move |captured: On<ScreenshotCaptured>| {
        *into.lock().unwrap() = Some(captured.image.clone());
    });
    let started = Instant::now();
    loop {
        app.update();
        if let Some(image) = slot.lock().unwrap().take() {
            return image;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "no capture within a minute");
    }
}

fn bytes(image: &Image) -> &[u8] {
    image.data.as_deref().expect("a captured image has its data")
}

/// The mean absolute difference between two pictures of one size, per colour byte (0 to 255).
fn mean_difference(a: &Image, b: &Image) -> f64 {
    assert_eq!(a.size(), b.size());
    let (a, b) = (bytes(a).as_chunks::<4>().0, bytes(b).as_chunks::<4>().0);
    let sum: u64 = a.iter().zip(b).flat_map(|(x, y)| (0..3).map(move |c| u64::from(x[c].abs_diff(y[c])))).sum();
    sum as f64 / (a.len() * 3) as f64
}

/// [`capture`] until two captures in a row are alike: pipelines compile a few frames in.
fn capture_stable(app: &mut App, image: &Handle<Image>) -> Image {
    let started = Instant::now();
    let mut before = capture(app, image.clone());
    loop {
        let now = capture(app, image.clone());
        if mean_difference(&before, &now) < 0.2 {
            return now;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "the picture never settled");
        before = now;
    }
}

/// The mean RGB of the `w` x `h` block at (`x`, `y`).
fn block(image: &Image, x: u32, y: u32, w: u32, h: u32) -> [f64; 3] {
    let (data, width) = (bytes(image), image.width() as usize);
    let mut sum = [0u64; 3];
    for row in y..y + h {
        for col in x..x + w {
            let at = (row as usize * width + col as usize) * 4;
            for (c, s) in sum.iter_mut().enumerate() {
                *s += u64::from(data[at + c]);
            }
        }
    }
    sum.map(|s| s as f64 / f64::from(w * h))
}

fn mean(image: &Image) -> [f64; 3] {
    block(image, 0, 0, image.width(), image.height())
}

fn luma([r, g, b]: [f64; 3]) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The grid block (of 16 x 9) whose brightness changed most between two pictures, and by how much (signed, `b` minus `a`).
fn most_changed_block(a: &Image, b: &Image) -> ((u32, u32), f64) {
    let (w, h) = (a.width() / 16, a.height() / 9);
    let mut best = ((0, 0), 0.0f64);
    for row in 0..9 {
        for col in 0..16 {
            let change = luma(block(b, col * w, row * h, w, h)) - luma(block(a, col * w, row * h, w, h));
            if change.abs() > best.1.abs() {
                best = ((col, row), change);
            }
        }
    }
    best
}

/// A `shot` of the game, so the pipelines are compiled before a raw capture is taken.
fn shot(app: &mut App, name: &str) {
    let path = format!("shots/{name}-{}.png", std::process::id());
    let ticket = ok(app, "shot", json!({"path": path})).result;
    let png = ticket["path"].as_str().unwrap().to_owned();
    let started = Instant::now();
    while !std::path::Path::new(&png).exists() {
        app.update();
        assert!(started.elapsed() < Duration::from_secs(120), "no screenshot within two minutes");
    }
    let _ = std::fs::remove_file(&png);
    let _ = std::fs::remove_file(ticket["legend_path"].as_str().unwrap());
}

/// Dusk: the sandbox's sun is dimmed and its ambient light turned down, so the room is lit by its own lamps and a
/// picture of it shows what they do.
fn dusk(app: &mut App) {
    let world = app.world_mut();
    let mut suns = world.query::<&mut DirectionalLight>();
    for mut sun in suns.iter_mut(world) {
        sun.illuminance = 600.0;
    }
    world.resource_mut::<GlobalAmbientLight>().brightness = 30.0;
}

/// The room, with the hero in it, drawn: pictures of it at the moments the tests choose.
fn lit_room_scene() -> (App, Handle<Image>) {
    let mut app = offscreen_app();
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    let picture = app.world().resource::<Offscreen>().image.clone();
    spawn_level_by_id(app.world_mut(), "lit_room", Some(Transform::from_translation(AT))).unwrap();
    dusk(&mut app);
    // Just left of the line that bounds the grading volume, facing the back wall.
    stand_at(&mut app, Vec3::new(-0.8, 0.0, 2.0));
    (app, picture)
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p sandbox --test sandbox -- --ignored lights"]
fn the_lamps_of_the_file_are_replaced_by_the_content_lights_in_the_picture() {
    let (mut app, _) = lit_room_scene();
    step(&mut app, 30);
    settle(&mut app, 120);
    let world = app.world_mut();
    let pictures = world.query::<&LightPicture>().iter(world).count();
    let points = world.query::<&PointLight>().iter(world).count();
    let spots = world.query::<&SpotLight>().iter(world).count();
    assert_eq!((pictures, points, spots), (6, 5, 1), "six content lights are all the picture has: the four lamps of the glTF are gone");
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p sandbox --test sandbox -- --ignored lights"]
fn a_flashing_lamp_lights_the_room_at_one_moment_and_not_at_another() {
    let (mut app, picture) = lit_room_scene();
    // Tick 30 is 0.5 s: the flasher's `m`, 400000 lm. Tick 90 is 1.5 s: its `a`, dark.
    step(&mut app, 30);
    settle(&mut app, 120);
    shot(&mut app, "lights-on");
    let on = capture_stable(&mut app, &picture);
    let again = capture(&mut app, picture.clone());
    assert!(mean_difference(&on, &again) < 0.5, "the same moment twice is the same picture: {}", mean_difference(&on, &again));
    step(&mut app, 60);
    settle(&mut app, 30);
    let off = capture_stable(&mut app, &picture);
    let (at, change) = most_changed_block(&on, &off);
    let (whole_on, whole_off) = (luma(mean(&on)), luma(mean(&off)));
    println!("flasher on: luma {whole_on:.2}, off: {whole_off:.2}; the block {at:?} changed by {change:.2}");
    assert!(change < -8.0, "the room is darker at the dark moment: the block {at:?} changed by {change:.2} (whole {whole_on:.2} → {whole_off:.2})");
    assert!(whole_on > whole_off + 0.2, "and the whole picture with it: {whole_on:.2} against {whole_off:.2}");
    // Two and a half seconds: the pattern has looped, the lamp is on again.
    step(&mut app, 60);
    settle(&mut app, 30);
    let back = capture_stable(&mut app, &picture);
    assert!((luma(mean(&back)) - whole_on).abs() < whole_on - whole_off, "back to the first moment's light: {:.2}", luma(mean(&back)));
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p sandbox --test sandbox -- --ignored lights"]
fn a_grading_volume_changes_the_colours_of_the_picture_and_leaving_it_gives_them_back() {
    let (mut app, picture) = lit_room_scene();
    // Tick 30: flasher on. The blend of `rust` takes 1.5 s; ticks 30 + 240 = 270 are 4.5 s, the flasher on again.
    step(&mut app, 30);
    settle(&mut app, 120);
    shot(&mut app, "grading-out");
    let before = capture_stable(&mut app, &picture);
    // One step to the right, into the volume.
    stand_at(&mut app, Vec3::new(0.8, 0.0, 2.0));
    step(&mut app, 240);
    settle(&mut app, 120);
    let after = capture_stable(&mut app, &picture);
    let (b, a) = (mean(&before), mean(&after));
    println!("outside: {b:.2?}, inside the volume: {a:.2?}");
    // `rust` is warmer (temperature +0.1), a little less saturated: red up, blue down.
    assert!(a[0] - a[2] > b[0] - b[2] + 6.0, "red against blue: {b:.2?} → {a:.2?}");
    // Back out: the colours return, over the same 1.5 s.
    stand_at(&mut app, Vec3::new(-0.8, 0.0, 2.0));
    step(&mut app, 240);
    settle(&mut app, 120);
    let again = capture_stable(&mut app, &picture);
    let c = mean(&again);
    assert!((c[0] - c[2] - (b[0] - b[2])).abs() < 4.0, "the first colours again: {b:.2?} against {c:.2?}");
}



/// A quarter of the manual's picture: `image` below the HUD (from row 190), at half size.
fn panel(image: &Image) -> (usize, usize, Vec<u8>) {
    const HUD: usize = 190;
    let stride = image.width() as usize;
    let (w, h) = (stride / 2, (image.height() as usize - HUD) / 2);
    let data = bytes(image);
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 3];
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let at = ((HUD + y * 2 + dy) * stride + x * 2 + dx) * 4;
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += u32::from(data[at + c]);
                }
            }
            out.extend(sum.map(|s| (s / 4) as u8));
        }
    }
    (w, h, out)
}

/// Four panels in two rows, a gap of 4 px between them, as one PNG at `path`.
fn write_sheet(path: &std::path::Path, panels: [&Image; 4]) {
    use shards::bevy::asset::RenderAssetUsages;
    use shards::bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    const GAP: usize = 4;
    let cells: Vec<(usize, usize, Vec<u8>)> = panels.iter().map(|p| panel(p)).collect();
    let (w, h) = (cells[0].0, cells[0].1);
    let (width, height) = (w * 2 + GAP, h * 2 + GAP);
    let mut rgba = vec![255u8; width * height * 4];
    for (i, (_, _, rgb)) in cells.iter().enumerate() {
        let (x0, y0) = ((i % 2) * (w + GAP), (i / 2) * (h + GAP));
        for y in 0..h {
            for x in 0..w {
                let (from, to) = ((y * w + x) * 3, ((y0 + y) * width + x0 + x) * 4);
                rgba[to..to + 3].copy_from_slice(&rgb[from..from + 3]);
            }
        }
    }
    let size = Extent3d { width: width as u32, height: height as u32, depth_or_array_layers: 1 };
    let sheet = Image::new(size, TextureDimension::D2, rgba, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    let picture = sheet.try_into_dynamic().expect("an RGBA8 image converts").to_rgb8();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    picture.save(path).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

#[test]
#[ignore = "needs a GPU, and writes only with SHARDS_WRITE_MEDIA=1: SHARDS_WRITE_MEDIA=1 cargo test -p sandbox --test sandbox -- --ignored the_manuals_picture"]
fn the_manuals_picture_of_the_room_is_written_on_request() {
    if std::env::var_os("SHARDS_WRITE_MEDIA").is_none() {
        return;
    }
    let (mut app, picture) = lit_room_scene();
    // Top row: the flashing lamp at 0.5 s (on) and 1.5 s (off). Bottom row: standing left of the volume's line, and right of it.
    step(&mut app, 30);
    settle(&mut app, 120);
    shot(&mut app, "media");
    let on = capture_stable(&mut app, &picture);
    step(&mut app, 60);
    settle(&mut app, 30);
    let off = capture_stable(&mut app, &picture);
    step(&mut app, 60);
    settle(&mut app, 30);
    let outside = capture_stable(&mut app, &picture);
    stand_at(&mut app, Vec3::new(0.8, 0.0, 2.0));
    step(&mut app, 240);
    settle(&mut app, 120);
    let inside = capture_stable(&mut app, &picture);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/media/lights.png");
    write_sheet(&path, [&on, &off, &outside, &inside]);
    println!("wrote {}", path.display());
}
