//! Effects on a GPU (`--offscreen`): a grey floor seen from above, the decal of a slam's
//! telegraph colouring it inside the zone and nowhere else, and glowing particles where an effect
//! plays, a level disc of its own colour on the floor as wide as its size; drawn into the offscreen image and read back pixel by
//! pixel.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use shards::ai_core::display::Offscreen;
use shards::ai_core::AiJournal;
use shards::bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use shards::bevy::render::view::{DebandDither, Msaa, Tonemapping};

use crate::*;

/// A glowing green spot that stays put for half a minute, and a magenta disc 2 m across that lies on the floor as long, in its
/// own colour (`unlit`) and over the floor as its alpha says.
const BEACON: &str = r##"{"beacon": (count: 6, speed: (0.0, 0.0), lifetime: (30.0, 30.0), size: (0.8, 0.8), size_end: 1.0,
    color: "#00ff00", color_end: "#00ff00", glow: true, max_live: 6),
    "pad": (count: 1, speed: (0.0, 0.0), lifetime: (30.0, 30.0), size: (2.0, 2.0), size_end: 1.0, shape: Disc,
    color: "#ff00ffcc", color_end: "#ff00ffcc", unlit: true, offset: (0.0, 0.05, 0.0), max_live: 1)}"##;

const SIZE: (u32, u32) = (320, 240);

/// The picture: each pixel's sRGB.
struct Picture {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 3]>,
}

impl Picture {
    fn read(image: &Image) -> Self {
        let (width, height) = (image.width(), image.height());
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let [r, g, b, _] = image.get_color_at(x, y).unwrap().to_srgba().to_u8_array();
                [r, g, b]
            })
            .collect();
        Self { width, height, pixels }
    }

    /// The pixels where `is` holds, as (x, y).
    fn find(&self, is: impl Fn([u8; 3]) -> bool) -> Vec<Vec2> {
        (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .filter(|(x, y)| is(self.pixels[(y * self.width + x) as usize]))
            .map(|(x, y)| Vec2::new(x as f32, y as f32))
            .collect()
    }
}

/// The floor: unlit grey.
fn floor_px([r, g, b]: [u8; 3]) -> bool {
    r.abs_diff(g) < 6 && g.abs_diff(b) < 6 && (100..=160).contains(&g)
}

/// Warm, as the decal of an enemy's attack tints the floor.
fn warm([r, g, b]: [u8; 3]) -> bool {
    r > g.saturating_add(30) && r > b.saturating_add(30)
}

fn green([r, g, b]: [u8; 3]) -> bool {
    g > r.saturating_add(50) && g > b.saturating_add(50)
}

fn magenta([r, g, b]: [u8; 3]) -> bool {
    r > g.saturating_add(80) && b > g.saturating_add(80)
}

fn mean(points: &[Vec2]) -> Vec2 {
    points.iter().copied().sum::<Vec2>() / points.len().max(1) as f32
}

/// The offscreen image once the GPU has drawn it.
fn capture(app: &mut App) -> Picture {
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
            return Picture::read(&image);
        }
        assert!(started.elapsed() < Duration::from_secs(60), "no screenshot within a minute");
    }
}

/// Captures until `done` holds of the picture (pipelines compile a few frames in).
fn capture_until(app: &mut App, what: &str, done: impl Fn(&Picture) -> bool) -> Picture {
    let started = Instant::now();
    loop {
        let picture = capture(app);
        if done(&picture) {
            return picture;
        }
        if started.elapsed() > Duration::from_secs(120) {
            let journal = app.world().resource::<AiJournal>();
            let errors: Vec<String> = journal.after(0).0.map(|e| e.text.clone()).filter(|t| t.contains("rror")).collect();
            panic!("{what} not drawn within two minutes; errors: {errors:#?}");
        }
    }
}

/// Where `at` is on the picture.
fn on_screen(app: &mut App, camera: Entity, at: Vec3) -> Vec2 {
    let world = app.world_mut();
    let mut query = world.query::<(&Camera, &GlobalTransform)>();
    let (camera, transform) = query.get(world, camera).unwrap();
    camera.world_to_viewport(transform, at).unwrap()
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p shards_fx --test fx -- --ignored offscreen"]
fn offscreen_a_telegraph_colours_the_floor_inside_its_zone_and_particles_glow() {
    let assets = Folder::new("offscreen");
    assets.put("content/fx/beacon.effect.ron", BEACON);
    let dir = assets.path().to_string_lossy().into_owned();
    let mut app = testing::test_app(
        move |config| {
            (config.headless, config.offscreen, config.window) = (false, true, SIZE);
            config.asset_dir = dir;
        },
        |app| {
            app.insert_resource(ClearColor(Color::srgb_u8(0, 0, 64)));
        },
    );
    let world = app.world_mut();
    floor(world);
    let plane = world.resource_mut::<Assets<Mesh>>().add(Plane3d::default().mesh().size(40.0, 40.0));
    let grey = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color: Color::srgb(0.5, 0.5, 0.5),
        unlit: true,
        ..default()
    });
    world.spawn((Mesh3d(plane), MeshMaterial3d(grey), Transform::default()));
    let brute = fighter(world, "brute", "monsters", Vec3::new(0.0, 0.0, -2.0), Vec3::Z, 100.0);
    // No anti-aliasing, tone curve or dither: a shade on screen is the shade the shader gave.
    let camera = (Camera3d::default(), AiCamera, Msaa::Off, Tonemapping::None, DebandDither::Disabled);
    let camera = world.spawn((camera, Transform::from_xyz(0.0, 12.0, 6.0).looking_at(Vec3::new(0.0, 0.0, -2.0), Vec3::Y))).id();

    let quiet = capture_until(&mut app, "the floor", |p| p.find(floor_px).len() > 20_000);
    assert!(quiet.find(warm).is_empty(), "no attack, nothing warm on the floor");

    // The brute's slam, half way to landing: the decal tints the floor in its 3 m circle.
    press(&mut app, brute, "light");
    step(&mut app, 24);
    let center = on_screen(&mut app, camera, Vec3::new(0.0, 0.0, -2.0));
    let rim: Vec<Vec2> = (0..16)
        .map(|i| {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            on_screen(&mut app, camera, Vec3::new(3.0 * ops::cos(a), 0.0, -2.0 + 3.0 * ops::sin(a)))
        })
        .collect();
    let (lo, hi) = rim.iter().fold((Vec2::MAX, Vec2::MIN), |(l, h), p| (l.min(*p), h.max(*p)));
    let inside = |p: &Vec2| p.x >= lo.x - 3.0 && p.x <= hi.x + 3.0 && p.y >= lo.y - 3.0 && p.y <= hi.y + 3.0;
    let area = ((hi.x - lo.x) * (hi.y - lo.y)) as usize;
    let tinted = capture_until(&mut app, "the decal", |p| p.find(warm).len() > area / 3);
    let warm_px = tinted.find(warm);
    let outside: Vec<&Vec2> = warm_px.iter().filter(|p| !inside(p)).collect();
    eprintln!(
        "decal: {} warm pixels, {} outside the zone's box {lo}..{hi}; centre {} (zone {center})",
        warm_px.len(),
        outside.len(),
        mean(&warm_px)
    );
    assert!(outside.len() < 20, "the decal stays in its zone: {} warm pixels outside", outside.len());
    assert!(mean(&warm_px).distance(center) < 12.0, "centred on the zone: {} vs {center}", mean(&warm_px));
    let said = ok(&mut app, "effects", json!({}));
    assert!(said.contains("telegraph decals on, on the ground: 1") && said.contains("slam: disc "), "{said}");

    // Off: the gizmos' thin lines alone, far fewer warm pixels.
    ok(&mut app, "telegraph_decals", json!({"on": false}));
    let lines = capture_until(&mut app, "the gizmo fill", |p| p.find(warm).len() < warm_px.len() / 3);
    eprintln!("decals off: {} warm pixels", lines.find(warm).len());
    ok(&mut app, "telegraph_decals", json!({"on": true}));

    // A glowing effect at a point beside the zone: green where it plays.
    let spot = Vec3::new(5.0, 1.0, -2.0);
    let said = ok(&mut app, "effect_play", json!({"effect": "beacon", "point": [5.0, 1.0, -2.0]}));
    assert!(said.starts_with("played beacon at a point (5.0, 1.0, -2.0): 6 particles"), "{said}");
    let glowing = capture_until(&mut app, "the particles", |p| p.find(green).len() > 50);
    let green_px = glowing.find(green);
    let at = on_screen(&mut app, camera, spot);
    eprintln!("particles: {} green pixels, centre {} (played at {at})", green_px.len(), mean(&green_px));
    assert!(mean(&green_px).distance(at) < 10.0, "{} vs {at}", mean(&green_px));
    let said = ok(&mut app, "effects", json!({}));
    assert!(said.contains("beacon") && said.contains("pool 6/6"), "{said}");

    // A disc on the floor: a level round patch of its colour, as wide as its size, where it plays.
    ok(&mut app, "effect_play", json!({"effect": "pad", "point": [-5.0, 0.0, -2.0]}));
    let padded = capture_until(&mut app, "the disc", |p| p.find(magenta).len() > 200);
    let pad_px = padded.find(magenta);
    let (left, right) = (on_screen(&mut app, camera, Vec3::new(-6.0, 0.05, -2.0)), on_screen(&mut app, camera, Vec3::new(-4.0, 0.05, -2.0)));
    let middle = on_screen(&mut app, camera, Vec3::new(-5.0, 0.05, -2.0));
    let span = pad_px.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p.x), h.max(p.x)));
    eprintln!("disc: {} magenta pixels, centre {}, across {:?} (2 m is {}..{})", pad_px.len(), mean(&pad_px), span, left.x, right.x);
    assert!(mean(&pad_px).distance(middle) < 6.0, "{} vs {middle}", mean(&pad_px));
    assert!((span.0 - left.x).abs() < 4.0 && (span.1 - right.x).abs() < 4.0, "2 m across: {span:?} vs {}..{}", left.x, right.x);
    assert!(padded.find(green).len() > 50, "the beacon still glows beside it");
}
