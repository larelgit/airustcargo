//! Pictures without a window (`--offscreen`): the GPU draws the sandbox into an image, the HUD and
//! the `eN` marks included, and `shot` saves it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shards::ai_core::display::{Offscreen, SceneImage};
use shards::ai_core::{AiCamera, AiJournal};
use shards::frames::FrameClock;
use shards::bevy::camera::RenderTarget;
use shards::bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{ok, step};

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1; Mesa's lavapipe on Linux): cargo test -p sandbox --test sandbox -- --ignored offscreen"]
fn an_offscreen_game_takes_shots_without_a_window() {
    let mut app = shards::testing::test_app(
        |config| (config.headless, config.offscreen) = (false, true),
        |app| {
            app.add_plugins((sandbox::SandboxPlugin, sandbox::visuals::VisualsPlugin));
        },
    );
    let display = shards::ai_core::status::status(app.world())["display"].clone();
    assert_eq!((display["mode"].as_str(), &display["size"]), (Some("offscreen"), &json!([1280, 720])), "{display}");
    assert!(display["gpu"].is_string(), "the GPU that draws is named: {display}");

    step(&mut app, 30);
    let ticket = ok(&mut app, "shot", json!({"path": format!("shots/offscreen-test-{}.png", std::process::id())})).result;
    let (png, legend) = (ticket["path"].as_str().unwrap().to_owned(), ticket["legend_path"].as_str().unwrap().to_owned());
    // The frame comes back a few frames later, once the GPU has compiled its pipelines.
    let started = Instant::now();
    let said = loop {
        app.update();
        let journal = app.world().resource::<AiJournal>();
        if let Some(line) = journal.after(0).0.map(|e| e.text.clone()).find(|t| t.starts_with("screenshot ")) {
            break line;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "no screenshot within two minutes");
    };
    assert_eq!(said, format!("screenshot saved: {png} (1280x720)"));

    let bytes = std::fs::read(&png).unwrap();
    let size = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    assert!(bytes.starts_with(b"\x89PNG") && (size(16), size(20)) == (1280, 720), "a 1280x720 PNG");
    assert!(bytes.len() > 20_000, "a picture, not an empty image: {} bytes", bytes.len());
    let legend: shards::serde_json::Value = shards::serde_json::from_str(&std::fs::read_to_string(&legend).unwrap()).unwrap();
    let marks: Vec<&str> = legend["legend"].as_array().unwrap().iter().filter_map(|l| l.as_str()).collect();
    assert!(marks.iter().any(|l| l.starts_with("e5 Player  px(")), "{marks:?}");
    let _ = std::fs::remove_file(&png);
    let _ = std::fs::remove_file(ticket["legend_path"].as_str().unwrap());
}

pub(crate) fn offscreen_app() -> App {
    shards::testing::test_app(
        |config| (config.headless, config.offscreen) = (false, true),
        |app| {
            app.add_plugins((sandbox::SandboxPlugin, sandbox::visuals::VisualsPlugin));
        },
    )
}

/// Frames drawn, so pipelines compile and the follow camera settles.
pub(crate) fn settle(app: &mut App, frames: u32) {
    for _ in 0..frames {
        app.update();
    }
}

/// `image` as the GPU drew it, read back.
pub(crate) fn capture(app: &mut App, image: Handle<Image>) -> Image {
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

/// [`capture`] until two captures in a row are alike: pipelines compile a few frames in (and, in a
/// debug build, many), and a picture taken before they are ready is not the world's.
pub(crate) fn capture_stable(app: &mut App, image: &Handle<Image>) -> Image {
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

/// The picture's colour bytes (RGBA, row by row).
pub(crate) fn bytes(image: &Image) -> &[u8] {
    image.data.as_deref().expect("a captured image has its data")
}

/// The mean absolute difference between two pictures of one size, per colour byte (0 to 255). The
/// alpha byte is left out: a screenshot drops it, and what the camera leaves in it is no part of
/// the picture.
pub(crate) fn mean_difference(a: &Image, b: &Image) -> f64 {
    assert_eq!(a.size(), b.size());
    let (a, b) = (bytes(a).as_chunks::<4>().0, bytes(b).as_chunks::<4>().0);
    let sum: u64 = a.iter().zip(b).flat_map(|(x, y)| (0..3).map(move |c| u64::from(x[c].abs_diff(y[c])))).sum();
    sum as f64 / (a.len() * 3) as f64
}

/// The mean RGB of the `w` x `h` block at (`x`, `y`).
pub(crate) fn block(image: &Image, x: u32, y: u32, w: u32, h: u32) -> [f64; 3] {
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

/// A `shot` of the game: the PNG's size and where `e5` is in it.
fn player_in_shot(app: &mut App, name: &str) -> ((u32, u32), Vec2) {
    let path = format!("shots/{name}-{}.png", std::process::id());
    let ticket = ok(app, "shot", json!({"path": path})).result;
    let (png, legend_path) = (ticket["path"].as_str().unwrap().to_owned(), ticket["legend_path"].as_str().unwrap().to_owned());
    let started = Instant::now();
    while !std::path::Path::new(&png).exists() {
        app.update();
        assert!(started.elapsed() < Duration::from_secs(120), "no screenshot within two minutes");
    }
    let legend: shards::serde_json::Value = shards::serde_json::from_str(&std::fs::read_to_string(&legend_path).unwrap()).unwrap();
    let size = (legend["width"].as_u64().unwrap() as u32, legend["height"].as_u64().unwrap() as u32);
    let line = legend["legend"].as_array().unwrap().iter().filter_map(|l| l.as_str()).find(|l| l.starts_with("e5 Player  px(")).expect("the player is in the picture").to_owned();
    let (x, y) = line.trim_start_matches("e5 Player  px(").split(')').next().unwrap().split_once(',').unwrap();
    let _ = std::fs::remove_file(&png);
    let _ = std::fs::remove_file(&legend_path);
    (size, Vec2::new(x.parse().unwrap(), y.parse().unwrap()))
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p sandbox --test sandbox -- --ignored render_scale"]
fn render_scale_draws_the_world_at_a_fraction_of_the_picture_and_shows_it_over_all_of_it() {
    let mut app = offscreen_app();
    let picture = app.world().resource::<Offscreen>().image.clone();
    step(&mut app, 30);
    settle(&mut app, 120);

    // At 1: the world is drawn into the picture itself.
    let camera = |app: &mut App| {
        let world = app.world_mut();
        let (camera, target) = world.query_filtered::<(&Camera, &RenderTarget), With<AiCamera>>().single(world).unwrap();
        (camera.physical_viewport_size(), camera.logical_viewport_size(), target.as_image().cloned())
    };
    let (physical, logical, target) = camera(&mut app);
    assert_eq!((physical, logical), (Some(UVec2::new(1280, 720)), Some(Vec2::new(1280.0, 720.0))));
    assert_eq!(target.as_ref(), Some(&picture));
    assert!(app.world().get_resource::<SceneImage>().is_none());
    let scene = shards::ai_core::status::status(app.world())["display"]["scene"].clone();
    assert_eq!(scene, json!({"size": [1280, 720], "scale": 1.0}));
    // A `shot` first: it waits for the pipelines to compile, which a raw capture does not (it would be the void).
    let (shot_size, player_whole) = player_in_shot(&mut app, "render-scale-1");
    let whole = capture_stable(&mut app, &picture);
    let counts = |app: &App| {
        let counts = shards::bench::counts(app.world());
        (counts["visible_meshes"].as_u64(), counts["camera_meshes"].as_u64())
    };
    let meshes_whole = counts(&app);
    assert_eq!(shot_size, (1280, 720));

    // `settings_set` applies it to the running game.
    ok(&mut app, "settings_set", json!({"group": "graphics", "key": "render_scale", "value": 0.5}));
    settle(&mut app, 60);
    assert_eq!(counts(&app), meshes_whole, "the camera that shows the scene sees no 3D mesh: the counters are the 3D camera's alone");
    let (physical, logical, target) = camera(&mut app);
    assert_eq!(physical, Some(UVec2::new(640, 360)), "the 3D target is half the picture's size");
    assert_eq!(logical, Some(Vec2::new(1280.0, 720.0)), "and its logical size the picture's");
    let scene_image = app.world().resource::<SceneImage>().image.clone();
    assert_eq!(target.as_ref(), Some(&scene_image), "the 3D camera draws into the scene image, not the picture");
    assert_eq!(app.world().resource::<Assets<Image>>().get(&picture).unwrap().size(), UVec2::new(1280, 720), "the picture is whole");
    let scene = shards::ai_core::status::status(app.world())["display"]["scene"].clone();
    assert_eq!(scene, json!({"size": [640, 360], "scale": 0.5}));
    let seen = capture(&mut app, scene_image.clone());
    assert_eq!((seen.width(), seen.height()), (640, 360), "the GPU's own image of the world");
    let half = capture_stable(&mut app, &picture);
    assert_eq!((half.width(), half.height()), (1280, 720));

    // The world fills the whole picture (not a corner of it), and looks like the world at 1.
    let difference = mean_difference(&whole, &half);
    assert!(difference < 12.0, "the picture at 0.5 differs from the one at 1 by {difference:.1} of 255 a byte");
    let corner = |image: &Image| block(image, 1200, 640, 60, 60);
    for (here, there) in corner(&half).iter().zip(corner(&whole)) {
        assert!((here - there).abs() < 25.0, "the bottom right corner: {:?} against {:?}", corner(&half), corner(&whole));
    }
    let (shot_size, player_half) = player_in_shot(&mut app, "render-scale-half");
    assert_eq!(shot_size, (1280, 720), "a shot is the picture's size, not the scene's");
    assert!(player_half.distance(player_whole) < 1.5, "e5 at {player_half} against {player_whole}: the marks are on the right pixels");

    // And back.
    ok(&mut app, "settings_set", json!({"group": "graphics", "key": "render_scale", "value": 1.0}));
    settle(&mut app, 60);
    let (physical, _, target) = camera(&mut app);
    assert_eq!((physical, target.as_ref()), (Some(UVec2::new(1280, 720)), Some(&picture)));
    assert!(app.world().get_resource::<SceneImage>().is_none());
    let again = capture_stable(&mut app, &picture);
    let difference = mean_difference(&whole, &again);
    assert!(difference < 1.0, "back at 1 the picture is the first one again: differs by {difference:.2}");
}

/// The draws of the newest frame the render world counted, once the game has drawn a while.
fn newest_draws(app: &mut App) -> u64 {
    settle(app, 40);
    app.world().resource::<FrameClock>().recent().filter_map(|f| f.draws).last().expect("the render world counted the draws of a frame")
}

fn visible_meshes(app: &App) -> u64 {
    shards::bench::counts(app.world())["visible_meshes"].as_u64().unwrap()
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p sandbox --test sandbox -- --ignored draws"]
fn draws_follow_how_bevy_batches_not_how_many_meshes_are_drawn() {
    use shards::bevy::render::render_resource::Face;
    const CUBES: u64 = 40;
    let mut app = offscreen_app();
    step(&mut app, 30);
    // A shot first: it waits for the pipelines, and draws that are skipped for lack of one are not the number.
    player_in_shot(&mut app, "draws");
    let (base, meshes) = (newest_draws(&mut app), visible_meshes(&app));
    assert!(base > 0, "a scene with a floor, a player and a HUD draws something");

    let at = crate::common::get::<Transform>(&app, "e5").translation;
    let cube = app.world_mut().resource_mut::<Assets<Mesh>>().add(Cuboid::new(0.25, 0.25, 0.25));
    // Forty cubes in a grid beside the player, each with a material of its own, made by `material(i)`.
    let add = |app: &mut App, material: &dyn Fn(u64) -> StandardMaterial| {
        let world = app.world_mut();
        let spawned: Vec<Entity> = (0..CUBES)
            .map(|i| {
                let at = at + Vec3::new((i % 8) as f32 * 0.4 - 1.4, 0.2, (i / 8) as f32 * 0.4 - 0.8);
                let material = world.resource_mut::<Assets<StandardMaterial>>().add(material(i));
                world.spawn((Mesh3d(cube.clone()), MeshMaterial3d(material), Transform::from_translation(at))).id()
            })
            .collect();
        let (draws, visible) = (newest_draws(app), visible_meshes(app));
        for cube in spawned {
            app.world_mut().despawn(cube);
        }
        (draws as i64 - base as i64, visible as i64 - meshes as i64)
    };

    // Forty meshes with forty materials of one pipeline are forty meshes more drawn and (on a GPU that
    // multi-draws, as Bevy 0.20 does here) no draw more: the count is of what is issued, not of what is shown.
    let (draws, visible) = add(&mut app, &|i| StandardMaterial::from(Color::srgb(0.9, 0.3 + i as f32 * 0.01, 0.2)));
    assert!(visible >= CUBES as i64 && draws.abs() <= 2, "{visible} meshes more drawn, {draws} draws more");

    // Four pipelines (lit or unlit, culled or not) are draws more, though not one for a mesh.
    let variety = |i: u64| StandardMaterial {
        unlit: i.is_multiple_of(2),
        cull_mode: if i % 4 < 2 { Some(Face::Back) } else { None },
        ..default()
    };
    let (draws, visible) = add(&mut app, &variety);
    assert!(visible >= CUBES as i64 && (4..CUBES as i64 / 2).contains(&draws), "{visible} meshes more drawn, {draws} draws more");

    // Alpha blending goes to the sorted phase.
    let (draws, _) = add(&mut app, &|i| StandardMaterial { base_color: Color::srgba(0.9, 0.3, 0.2, 0.5), alpha_mode: AlphaMode::Blend, perceptual_roughness: 0.3 + i as f32 * 0.01, ..default() });
    assert!(draws >= 1 && draws < CUBES as i64 / 2, "{draws} draws more for blended cubes");
    assert_eq!(newest_draws(&mut app), base, "the cubes gone, the scene's draws are as they were");

    let frames = shards::ai_core::status::status(app.world())["frames"].clone();
    assert!(frames["draws"].as_f64().is_some_and(|d| d >= base as f64) && frames["draws_max"].as_u64().is_some_and(|d| d > base), "{frames}");
}

/// Where the player's model is drawn (world x), and what the player's visual child says.
fn model_x(app: &mut App) -> (f32, Option<f32>) {
    let world = app.world_mut();
    let mut q = world.query::<(&shards::motion::Interpolated, &GlobalTransform, &ChildOf)>();
    let bodies: Vec<_> = q.iter(world).map(|(i, g, p)| (i.shown().map(|s| s.translation.x), g.translation().x, p.parent())).collect();
    let (shown, x, _) = bodies
        .into_iter()
        .find(|(_, _, parent)| world.get::<sandbox::gameplay::Player>(*parent).is_some())
        .expect("the player's model is an interpolated child");
    (x, shown)
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p sandbox --test sandbox -- --ignored the_model_moves"]
fn the_model_moves_between_ticks_while_the_body_stays_on_the_tick() {
    use std::time::Duration;
    let mut app = offscreen_app();
    step(&mut app, 30);
    ok(&mut app, "move", json!({"dir": "E", "meters": 8}));
    step(&mut app, 20);
    settle(&mut app, 120);
    // A running clock that adds no time: frames, no ticks, and the overstep is set by hand.
    app.insert_resource(shards::bevy::time::TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    shards::ai_core::sim::resume(app.world_mut());
    let at = |app: &mut App, fraction: f64| {
        let step = app.world().resource::<Time<Fixed>>().timestep();
        let mut fixed = app.world_mut().resource_mut::<Time<Fixed>>();
        let pending = fixed.overstep();
        fixed.discard_overstep(pending);
        fixed.accumulate_overstep(step.mul_f64(fraction));
        app.update();
        model_x(app)
    };
    let body = crate::common::get::<Transform>(&app, "e5").translation.x;
    let (older, _) = at(&mut app, 0.0);
    let (half, shown) = at(&mut app, 0.5);
    let (last, _) = at(&mut app, 0.99);
    assert!(older < half && half < last, "the model moves between ticks: {older} < {half} < {last}");
    assert!(shown.is_some_and(|x| (x - (older + last) / 2.0).abs() < 0.05), "{shown:?}");
    assert_eq!(crate::common::get::<Transform>(&app, "e5").translation.x, body, "the body is never touched");

    // Off: the model stays where the tick left it, however far into the next one the frame is.
    ok(&mut app, "settings_set", json!({"group": "graphics", "key": "interpolation", "value": false}));
    let (a, _) = at(&mut app, 0.0);
    let (b, shown) = at(&mut app, 0.5);
    assert_eq!(a, b);
    assert!(shown.is_none());
}

/// What the simulation reads does not depend on whether anything is drawn: the same session
/// headless (as `--headless` builds it: no visuals) and drawn offscreen (models, animation,
/// effects, foliage, the tiles' visuals, the HUD) reads the same definitions, so the replay check
/// `content` and the recordings' parts are the same in both.
#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1; Mesa's lavapipe on Linux): cargo test -p sandbox --test sandbox -- --ignored offscreen"]
fn content_reads_are_the_same_headless_and_drawn() {
    use shards::ai_core::replay;
    use shards::content::ContentDb;

    // Ticks in short runs with frames between, as a live game has them: a drawn one spawns
    // models, animates them, streams the tiles' visuals, scatters foliage and plays effects there.
    let run = |app: &mut App, ticks: u64| {
        for _ in 0..ticks / 20 {
            step(app, 20);
            settle(app, 3);
        }
    };
    let session = |app: &mut App| {
        settle(app, 10);
        ok(app, "teleport", json!({"id": "e5", "x": 0, "z": -3}));
        ok(app, "attack", json!({"target": "e6", "repeat": true}));
        run(app, 240);
        shards::testing::call(app, "dodge", json!({}));
        run(app, 120);
        ok(app, "regen", json!({"region": "wilds", "seed": 7}));
        settle(app, 10);
        run(app, 240);
        ok(app, "ui_open", json!({"screen": "pause"}));
        settle(app, 3);
        ok(app, "ui_navigate", json!({"dir": "back"}));
        run(app, 60);
        let read = app.world().resource::<ContentDb>().used_parts();
        (replay::save(app.world()), read)
    };
    let mut headless = sandbox::test_app(42);
    let mut drawn = shards::testing::test_app(
        |config| (config.headless, config.offscreen, config.seed) = (false, true, 42),
        |app| {
            app.add_plugins((sandbox::SandboxPlugin, sandbox::visuals::VisualsPlugin));
        },
    );
    let display = shards::ai_core::status::status(drawn.world())["display"].clone();
    assert_eq!(display["mode"].as_str(), Some("offscreen"), "{display}");
    let (plain, plain_read) = session(&mut headless);
    let (pictured, pictured_read) = session(&mut drawn);
    let meshes = |app: &mut App| app.world_mut().query::<&Mesh3d>().iter(app.world()).count();
    let (none, many) = (meshes(&mut headless), meshes(&mut drawn));
    assert!(many > none + 1000, "the drawn game draws (models, tiles, foliage): {many} meshes, headless {none}");
    let names = |read: &[(String, u64)]| read.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert!(names(&plain_read).iter().any(|n| n == "tile (all)") && names(&plain_read).iter().any(|n| n == "action/light_1"), "{:?}", names(&plain_read));
    assert_eq!(names(&pictured_read), names(&plain_read));
    assert_eq!(pictured_read, plain_read);
    assert_eq!(pictured.check_parts, plain.check_parts);
    let hashes = |r: &replay::Replay| r.checks.iter().map(|c| (c.tick, c.hash)).collect::<Vec<_>>();
    assert_eq!(hashes(&pictured), hashes(&plain));
    assert_eq!(pictured.end.hash, plain.end.hash);
}
