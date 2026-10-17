//! The looks on a GPU (`--offscreen`): a toon sphere with an outline beside a lit one, drawn into
//! the offscreen image and read back pixel by pixel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use shards::ai_core::display::Offscreen;
use shards::ai_core::AiJournal;
use shards::bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use shards::bevy::render::view::{DebandDither, Msaa, Tonemapping};
use shards::look::{Look, Looked, ToonMaterial};

use crate::*;

/// Grey in three bands with a magenta outline 3 px wide; the same grey lit smoothly.
const SPHERES: &str = r##"{
    "toon": (color: Some("#c0c0c0"), shading: Toon(bands: 3), outline: Some((width: 3.0, color: "#ff00ff"))),
    "smooth": (color: Some("#c0c0c0"), shading: Lit(roughness: 1.0, metallic: 0.0)),
}"##;

const SIZE: (u32, u32) = (320, 240);

/// What a pixel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Px {
    Background,
    Outline,
    Surface([u8; 3]),
}

/// The picture, row by row.
struct Picture {
    width: usize,
    pixels: Vec<Px>,
}

impl Picture {
    fn read(image: &Image) -> Self {
        let (width, height) = (image.width(), image.height());
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let [r, g, b, _] = image.get_color_at(x, y).unwrap().to_srgba().to_u8_array();
                if r < 12 && g < 12 && (52..=76).contains(&b) {
                    Px::Background
                } else if r > 200 && g < 60 && b > 200 {
                    Px::Outline
                } else {
                    Px::Surface([r, g, b])
                }
            })
            .collect();
        Self { width: width as usize, pixels }
    }

    fn at(&self, x: usize, y: usize) -> Px {
        self.pixels[y * self.width + x]
    }

    fn height(&self) -> usize {
        self.pixels.len() / self.width
    }

    /// The pixels of the left (`false`) or right half that are not background, as (x, y, px).
    fn half(&self, right: bool) -> Vec<(usize, usize, Px)> {
        let xs = if right { self.width / 2..self.width } else { 0..self.width / 2 };
        (0..self.height())
            .flat_map(|y| xs.clone().map(move |x| (x, y)))
            .map(|(x, y)| (x, y, self.at(x, y)))
            .filter(|(_, _, px)| *px != Px::Background)
            .collect()
    }

    /// Whether both spheres and the outline are drawn: the pipelines are ready.
    fn complete(&self) -> bool {
        let (left, right) = (self.half(false), self.half(true));
        let surface = |half: &[(usize, usize, Px)]| half.iter().filter(|(_, _, px)| matches!(px, Px::Surface(_))).count() > 1000;
        surface(&left) && surface(&right) && left.iter().any(|(_, _, px)| *px == Px::Outline)
    }
}

/// A sphere in one half: its centre and radius in pixels (outline included), from what is drawn.
fn disc(drawn: &[(usize, usize, Px)]) -> (usize, usize, usize) {
    let (xs, ys) = (drawn.iter().map(|p| p.0), drawn.iter().map(|p| p.1));
    let (left, right, top, bottom) = (xs.clone().min().unwrap(), xs.max().unwrap(), ys.clone().min().unwrap(), ys.max().unwrap());
    ((left + right) / 2, (top + bottom) / 2, (right - left) / 2)
}

/// The share of the surface's pixels within 70% of the radius that the `n` most common shades cover.
fn banded(drawn: &[(usize, usize, Px)], n: usize) -> (f32, usize) {
    let (cx, cy, radius) = disc(drawn);
    let inner = (radius * 7 / 10) as f32;
    let mut shades: HashMap<[u8; 3], usize> = HashMap::new();
    for &(x, y, px) in drawn {
        if let Px::Surface(rgb) = px
            && (x as f32 - cx as f32).hypot(y as f32 - cy as f32) < inner
        {
            *shades.entry(rgb).or_default() += 1;
        }
    }
    let total: usize = shades.values().sum();
    let mut counts: Vec<usize> = shades.values().copied().collect();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    (counts.iter().take(n).sum::<usize>() as f32 / total as f32, shades.len())
}

/// How many outline pixels come first from the edge of the drawn sphere inwards, along `step`.
fn outline_from(picture: &Picture, (x, y): (usize, usize), step: (isize, isize)) -> usize {
    let (mut x, mut y) = (x as isize, y as isize);
    // From the background into the sphere.
    while picture.at(x as usize, y as usize) == Px::Background {
        (x, y) = (x + step.0, y + step.1);
    }
    let mut run = 0;
    while picture.at(x as usize, y as usize) == Px::Outline {
        run += 1;
        (x, y) = (x + step.0, y + step.1);
    }
    assert!(matches!(picture.at(x as usize, y as usize), Px::Surface(_)), "the surface comes after the outline at ({x}, {y})");
    run
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

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p shards_look --test look -- --ignored offscreen"]
fn offscreen_a_toon_sphere_is_drawn_in_bands_inside_its_outline() {
    let assets = Folder::empty("offscreen");
    assets.put("content/look/spheres.look.ron", SPHERES);
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
    let sphere = world.resource_mut::<Assets<Mesh>>().add(Sphere::new(1.0).mesh().uv(64, 32));
    let white = world.resource_mut::<Assets<StandardMaterial>>().add(Color::WHITE);
    for (x, look) in [(-1.3, "toon"), (1.3, "smooth")] {
        world.spawn((Mesh3d(sphere.clone()), MeshMaterial3d(white.clone()), Transform::from_xyz(x, 0.0, 0.0), Look::new(look)));
    }
    let sun = DirectionalLight { illuminance: 2000.0, ..default() };
    world.spawn((sun, Transform::from_xyz(-3.0, 4.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y)));
    // No anti-aliasing, tone curve or dither: a shade on screen is the shade the shader gave.
    let camera = (Camera3d::default(), Msaa::Off, Tonemapping::None, DebandDither::Disabled);
    world.spawn((camera, Transform::from_xyz(0.0, 0.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y)));

    // The pipelines compile a few frames in.
    let started = Instant::now();
    let picture = loop {
        let picture = Picture::read(&capture(&mut app));
        if picture.complete() {
            break picture;
        }
        if started.elapsed() > Duration::from_secs(120) {
            let journal = app.world().resource::<AiJournal>();
            let errors: Vec<String> = journal.after(0).0.map(|e| e.text.clone()).filter(|t| t.contains("rror")).collect();
            panic!("the spheres and the outline were not drawn within two minutes; errors: {errors:#?}");
        }
    };
    let said = ok(&mut app, "looks", json!({}));
    assert!(said.starts_with("2 looks · 2 meshes styled (1 outlined) · 0 without a look · outlines on\n"), "{said}");

    // The outline: 3 px of magenta around the toon sphere, on every side; none around the other.
    let (toon, smooth) = (picture.half(false), picture.half(true));
    let (cx, cy, _) = disc(&toon);
    let half_x = picture.width / 2;
    let runs = [
        outline_from(&picture, (0, cy), (1, 0)),
        outline_from(&picture, (half_x - 1, cy), (-1, 0)),
        outline_from(&picture, (cx, 0), (0, 1)),
        outline_from(&picture, (cx, picture.height() - 1), (0, -1)),
    ];
    assert!(runs.iter().all(|run| (2..=5).contains(run)), "outline widths left, right, top, bottom: {runs:?}");
    assert!(!smooth.iter().any(|(_, _, px)| *px == Px::Outline), "the lit sphere has no outline");
    let (sx, sy, _) = disc(&smooth);
    assert_eq!(outline_from(&picture, (half_x, sy), (1, 0)), 0);
    assert_eq!(outline_from(&picture, (sx, 0), (0, 1)), 0);

    // Toon light: a few flat shades; the lit sphere's light runs smoothly over many.
    let (toon_share, toon_shades) = banded(&toon, 3);
    let (smooth_share, smooth_shades) = banded(&smooth, 3);
    eprintln!("toon: 3 shades cover {toon_share:.3} of {toon_shades}; lit: {smooth_share:.3} of {smooth_shades}; outline {runs:?}");
    assert!(toon_share > 0.9, "the toon sphere's 3 commonest shades cover {toon_share} of its {toon_shades}");
    assert!(smooth_share < 0.3, "the lit sphere's 3 commonest shades cover {smooth_share} of its {smooth_shades}");

    // Outlines off: the toon sphere without its line; back on, it is there again.
    ok(&mut app, "look_outlines", json!({"on": false}));
    let hidden = Picture::read(&capture(&mut app));
    assert!(!hidden.half(false).iter().any(|(_, _, px)| *px == Px::Outline), "no outline while they are off");
    assert!(hidden.half(false).len() > 1000, "the sphere is still drawn");
    ok(&mut app, "look_outlines", json!({"on": true}));
    let shown = Picture::read(&capture(&mut app));
    assert!(shown.half(false).iter().any(|(_, _, px)| *px == Px::Outline));

    // Drawing restyles nothing (the render world takes the looks' meshes and materials
    // untracked); the game's material changed in place restyles both spheres once.
    let styled = |app: &mut App| {
        let world = app.world_mut();
        let mut query = world.query::<(&Looked, Option<&MeshMaterial3d<ToonMaterial>>, Option<&MeshMaterial3d<StandardMaterial>>)>();
        let mut styled: Vec<String> =
            query.iter(world).map(|(l, t, s)| format!("{} {:?} {:?} {:?}", l.look, l.hull, t.map(|m| m.0.id()), s.map(|m| m.0.id()))).collect();
        styled.sort();
        styled
    };
    let before = styled(&mut app);
    capture(&mut app);
    assert_eq!(styled(&mut app), before, "drawing restyles nothing");
    app.world_mut().resource_mut::<Assets<StandardMaterial>>().get_mut(&white).unwrap().base_color = Color::srgb(1.0, 0.5, 0.5);
    capture(&mut app);
    let after = styled(&mut app);
    assert!(after.iter().zip(&before).all(|(a, b)| a != b), "both restyled: {before:?} -> {after:?}");
    capture(&mut app);
    assert_eq!(styled(&mut app), after, "once");
}
