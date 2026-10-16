//! The scatter on a GPU (`--offscreen`): the test region's tufts in two toon greens, seen from
//! among them, drawn into the offscreen image and read back pixel by pixel; then taken away, then
//! seen from beyond their last level of detail.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use shards::ai_core::display::Offscreen;
use shards::bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use shards::bevy::render::view::{DebandDither, Msaa, Tonemapping};

use crate::*;

const SIZE: (u32, u32) = (320, 240);

/// Toon greens (3 bands) instead of the matte ones: the toon shader dissolves levels of detail too.
const TOON_LOOKS: &str = r##"{
    "green": (color: Some("#2f8f3a")),
    "olive": (color: Some("#7a8f2f")),
    "grey": (color: Some("#808080"), flat: true),
}"##;

/// The tufts alone: every pixel drawn is one of theirs.
const TUFTS: &str = r##"{
    "tufts": (
        density: 1.0,
        spacing: 0.3,
        scale: (0.8, 1.2),
        tilt: 10.0,
        looks: ["green", "olive"],
        fade: 2.0,
        lods: [
            (mesh: Blades(width: 0.08, height: 0.4, count: 5), until: 20.0),
            (mesh: Blades(width: 0.12, height: 0.4, count: 2), until: 40.0),
        ],
    ),
}"##;

/// What the picture shows, in pixels.
#[derive(Debug, Default)]
struct Picture {
    /// The clear colour.
    background: usize,
    /// Green the most of the three channels: the tufts.
    green: usize,
    /// Anything else.
    other: usize,
    /// The mean colour of the green pixels.
    mean: [f32; 3],
}

impl Picture {
    fn read(image: &Image) -> Self {
        let mut picture = Picture::default();
        let mut sum = [0.0f32; 3];
        for y in 0..image.height() {
            for x in 0..image.width() {
                let [r, g, b, _] = image.get_color_at(x, y).unwrap().to_srgba().to_u8_array();
                if r < 12 && g < 12 && (52..=76).contains(&b) {
                    picture.background += 1;
                } else if g > r && g > b {
                    picture.green += 1;
                    for (s, c) in sum.iter_mut().zip([r, g, b]) {
                        *s += f32::from(c);
                    }
                } else {
                    picture.other += 1;
                }
            }
        }
        picture.mean = sum.map(|s| s / picture.green.max(1) as f32);
        picture
    }
}

/// The offscreen image once the GPU has drawn it.
fn capture(app: &mut App) -> Image {
    let image = app.world().resource::<Offscreen>().image.clone();
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
        assert!(started.elapsed() < Duration::from_secs(60), "no screenshot within a minute");
    }
}

/// Captures until `done` holds (pipelines compile a few frames in), at most two minutes.
fn picture_until(app: &mut App, what: &str, done: impl Fn(&Picture) -> bool) -> Picture {
    let started = Instant::now();
    loop {
        let picture = Picture::read(&capture(app));
        if done(&picture) {
            return picture;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "{what} within two minutes: {picture:?}");
    }
}

fn look_from(app: &mut App, at: Vec3) {
    let target = Vec3::new(8.0, 0.0, 8.0);
    let mut cameras = app.world_mut().query_filtered::<&mut Transform, With<Camera3d>>();
    for mut camera in cameras.iter_mut(app.world_mut()) {
        *camera = Transform::from_translation(at).looking_at(target, Vec3::Y);
    }
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p shards_scatter --test scatter -- --ignored offscreen"]
fn offscreen_tufts_are_drawn_green_in_batches_and_not_beyond_their_last_level() {
    let assets = Folder::new("offscreen");
    assets.put("content/look/test.look.ron", TOON_LOOKS);
    assets.put("content/world/test.scatter.ron", TUFTS);
    let dir = assets.path().to_string_lossy().into_owned();
    let mut app = testing::test_app(
        move |config| {
            (config.headless, config.offscreen, config.window) = (false, true, SIZE);
            config.asset_dir = dir;
        },
        |app| {
            app.start_region("plain");
            app.insert_resource(ClearColor(Color::srgb_u8(0, 0, 64)));
        },
    );
    let world = app.world_mut();
    let sun = DirectionalLight { illuminance: 3000.0, ..default() };
    world.spawn((sun, Transform::from_xyz(-3.0, 10.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y)));
    // No anti-aliasing, tone curve or dither: a shade on screen is the shade the shader gave.
    world.spawn((Camera3d::default(), Msaa::Off, Tonemapping::None, DebandDither::Disabled, Transform::default()));
    // The field tile (centered at (8, 0, 8)) from the mound tile south of it, 6 m up: tufts near
    // and far, both levels of detail.
    look_from(&mut app, Vec3::new(8.0, 6.0, 22.0));

    let near = picture_until(&mut app, "the tufts are drawn", |p| p.green > 2000);
    let [r, g, b] = near.mean;
    assert!(g > r * 1.15 && g > b * 1.6, "the looks' greens (#2f8f3a, #7a8f2f), lit: mean {:?}", near.mean);
    assert!(near.background > near.green, "the ground is not drawn, only what is strewn on it: {near:?}");
    assert!(near.other * 10 < near.green, "nearly every drawn pixel is green: {near:?}");
    let counts = shards::ai_core::status::status(app.world())["counts"].clone();
    let (visible, assets) = (counts["visible_meshes"].as_u64().unwrap(), counts["mesh_assets"].as_u64().unwrap());
    assert!(visible > 300 && assets <= 2, "hundreds of instances drawn, from two meshes (one per level): {counts}");

    // Taken away: nothing green is left.
    let said = ok(&mut app, "scatter_set", json!({"on": false}));
    assert_eq!(said, "scatter off: 0 instances on 0 tiles");
    let off = picture_until(&mut app, "the tufts are gone", |p| p.green == 0);
    assert_eq!(off.other, 0, "{off:?}");

    // Back, and seen from beyond 40 m, where the tufts' last level of detail ends: nothing is drawn.
    ok(&mut app, "scatter_set", json!({"on": true}));
    look_from(&mut app, Vec3::new(8.0, 20.0, 85.0));
    let far = picture_until(&mut app, "the tufts are culled far away", |p| p.green == 0 && p.other == 0);
    assert_eq!(far.background, (SIZE.0 * SIZE.1) as usize);
    // And from near again, they come back.
    look_from(&mut app, Vec3::new(8.0, 6.0, 22.0));
    picture_until(&mut app, "the tufts come back near", |p| p.green > 2000);
}
