//! Writes the template's loading picture, `template/assets/ui/loading.png` (its main menu shows it
//! too): drawn here from flat shapes, so the repository holds no one else's art. Run it again
//! after changing the scene:
//!
//! ```text
//! cargo run -p sandbox --example make_art
//! ```
//!
//! A dusk in the engine's own look, light in bands and flat faces: a sky in bands, a low sun, three
//! ridges of hills, a lone tower with a lit window, pines. No text: the picture is the same in every
//! language. 1280 × 720 (16:9), drawn at twice that and averaged down, so edges are smooth; flat
//! colours keep the PNG small. The stars are seeded: the file comes out the same every time.

use std::path::{Path, PathBuf};

use shards::bevy::asset::RenderAssetUsages;
use shards::bevy::image::Image;
use shards::bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
/// Samples per pixel along each side.
const SS: usize = 2;

type Rgb = [f32; 3];

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let template = root.parent().and_then(Path::parent).expect("crates/sandbox is two folders down").join("template/assets");
    let mut canvas = Canvas::new();
    dusk(&mut canvas);
    write(&canvas, &template.join("ui/loading.png"));
}

/// The scene, back to front, in the picture's pixels (y down).
fn dusk(c: &mut Canvas) {
    // The sky in bands, deep blue above to amber at the horizon.
    let sky = ["1b1d3a", "252450", "362b5c", "4f3363", "6e3b66", "934563", "b9535c", "d96a55", "ee8a52", "f6ad62"];
    let band = 470.0 / sky.len() as f32;
    for (i, color) in sky.iter().enumerate() {
        c.rect(0.0, i as f32 * band, WIDTH as f32, (i + 1) as f32 * band + 1.0, hex(color), 1.0);
    }
    c.rect(0.0, 470.0, WIDTH as f32, HEIGHT as f32, hex("f6ad62"), 1.0);

    // Stars in the upper bands.
    let mut rng = Lcg(0x5EED_A127);
    for _ in 0..70 {
        let (x, y) = (rng.next() * WIDTH as f32, rng.next() * 230.0);
        let r = 0.8 + rng.next() * 1.2;
        c.circle(x, y, r, hex("fff3d6"), 0.35 + 0.5 * (1.0 - y / 230.0));
    }

    // The sun, low over the far hills, its glow in two rings.
    let (sx, sy) = (900.0, 440.0);
    c.circle(sx, sy, 150.0, hex("ffd59a"), 0.12);
    c.circle(sx, sy, 105.0, hex("ffd59a"), 0.2);
    c.circle(sx, sy, 68.0, hex("ffe2a8"), 1.0);

    // Far hills, lit on the faces turned to the sun (right), shadowed on the others.
    let far = [
        (0.0, 430.0), (70.0, 395.0), (140.0, 420.0), (230.0, 360.0), (320.0, 410.0), (400.0, 385.0), (470.0, 425.0), (560.0, 350.0),
        (650.0, 415.0), (740.0, 390.0), (820.0, 440.0), (930.0, 400.0), (1010.0, 430.0), (1100.0, 370.0), (1190.0, 415.0), (1280.0, 395.0),
    ];
    c.ridge(&far, 720.0, hex("6a4470"), hex("54375f"));
    let mid = [
        (0.0, 505.0), (90.0, 470.0), (190.0, 500.0), (300.0, 455.0), (420.0, 510.0), (520.0, 480.0), (610.0, 515.0), (720.0, 470.0),
        (840.0, 520.0), (960.0, 485.0), (1060.0, 525.0), (1170.0, 475.0), (1280.0, 505.0),
    ];
    c.ridge(&mid, 720.0, hex("472f5a"), hex("3a284d"));

    // The near hill the tower stands on, rising on the left.
    let near = [
        (0.0, 560.0), (110.0, 535.0), (220.0, 528.0), (330.0, 532.0), (450.0, 556.0), (560.0, 590.0), (700.0, 615.0), (860.0, 628.0),
        (1020.0, 610.0), (1160.0, 640.0), (1280.0, 625.0),
    ];
    c.ridge(&near, 720.0, hex("2b2242"), hex("231c37"));

    // The tower: a body, battlements, a pointed roof, one lit window and a slit.
    let stone = hex("17132b");
    c.rect(300.0, 380.0, 360.0, 540.0, stone, 1.0);
    c.rect(292.0, 368.0, 368.0, 384.0, stone, 1.0);
    for i in 0..4 {
        let x = 292.0 + i as f32 * 20.0;
        c.rect(x, 356.0, x + 10.0, 370.0, stone, 1.0);
    }
    c.polygon(&[(300.0, 356.0), (330.0, 300.0), (360.0, 356.0)], hex("221a3a"), 1.0);
    c.polygon(&[(330.0, 300.0), (360.0, 356.0), (330.0, 356.0)], hex("1b1531"), 1.0);
    c.rect(323.0, 282.0, 325.0, 300.0, stone, 1.0);
    c.polygon(&[(325.0, 282.0), (346.0, 288.0), (325.0, 294.0)], hex("c9563f"), 1.0);
    c.rect(324.0, 404.0, 338.0, 426.0, hex("ffcf6e"), 1.0);
    c.rect(328.0, 470.0, 332.0, 492.0, hex("2c2346"), 1.0);

    // Pines along the near hill.
    let pines = [(150.0, 540.0, 46.0), (190.0, 534.0, 60.0), (420.0, 552.0, 52.0), (470.0, 560.0, 38.0), (980.0, 615.0, 56.0), (1035.0, 612.0, 44.0), (1210.0, 636.0, 50.0)];
    for (x, ground, h) in pines {
        pine(c, x, ground, h);
    }
}

/// A pine of height `h` standing at `x` on `ground`: three tiers, shadowed on the left.
fn pine(c: &mut Canvas, x: f32, ground: f32, h: f32) {
    let w = h * 0.42;
    c.rect(x - 2.0, ground - h * 0.18, x + 2.0, ground + 4.0, hex("120f22"), 1.0);
    for tier in 0..3 {
        let t = tier as f32;
        let base = ground - h * (0.12 + 0.26 * t);
        let top = base - h * 0.5;
        let half = w * (1.0 - 0.22 * t);
        c.polygon(&[(x - half, base), (x, top), (x + half, base)], hex("1d1830"), 1.0);
        c.polygon(&[(x - half, base), (x, top), (x, base)], hex("151124"), 1.0);
    }
}

/// The picture at `SS` × its size, colours in 0..1.
struct Canvas {
    px: Vec<Rgb>,
}

impl Canvas {
    const W: usize = WIDTH * SS;
    const H: usize = HEIGHT * SS;

    fn new() -> Self {
        Self { px: vec![[0.0; 3]; Self::W * Self::H] }
    }

    fn blend(&mut self, x: usize, y: usize, color: Rgb, alpha: f32) {
        let p = &mut self.px[y * Self::W + x];
        for (dst, src) in p.iter_mut().zip(color) {
            *dst += (src - *dst) * alpha;
        }
    }

    /// The samples whose centres lie in `a..b` (picture pixels), up to `count` of them.
    fn span(a: f32, b: f32, count: usize) -> std::ops::Range<usize> {
        let first = (a * SS as f32 - 0.5).ceil().max(0.0) as usize;
        let last = ((b * SS as f32 - 0.5).ceil().max(0.0) as usize).min(count);
        first..last
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Rgb, alpha: f32) {
        for y in Self::span(y0, y1, Self::H) {
            for x in Self::span(x0, x1, Self::W) {
                self.blend(x, y, color, alpha);
            }
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgb, alpha: f32) {
        for y in Self::span(cy - r, cy + r, Self::H) {
            let dy = (y as f32 + 0.5) / SS as f32 - cy;
            let half = (r * r - dy * dy).max(0.0).sqrt();
            for x in Self::span(cx - half, cx + half, Self::W) {
                self.blend(x, y, color, alpha);
            }
        }
    }

    /// A simple polygon (even-odd), by scanlines.
    fn polygon(&mut self, points: &[(f32, f32)], color: Rgb, alpha: f32) {
        let top = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let bottom = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        for y in Self::span(top, bottom, Self::H) {
            let sy = (y as f32 + 0.5) / SS as f32;
            let mut xs: Vec<f32> = Vec::new();
            for (i, &(ax, ay)) in points.iter().enumerate() {
                let (bx, by) = points[(i + 1) % points.len()];
                if (ay <= sy) != (by <= sy) {
                    xs.push(ax + (sy - ay) / (by - ay) * (bx - ax));
                }
            }
            xs.sort_by(f32::total_cmp);
            for [a, b] in xs.as_chunks::<2>().0 {
                for x in Self::span(*a, *b, Self::W) {
                    self.blend(x, y, color, alpha);
                }
            }
        }
    }

    /// A ridge of hills down to `floor`, in `shade`, and on every slope that falls to the right
    /// (towards the sun) a flat face in `lit` reaching down into the hill.
    fn ridge(&mut self, points: &[(f32, f32)], floor: f32, lit: Rgb, shade: Rgb) {
        let mut body = points.to_vec();
        body.extend([(points[points.len() - 1].0, floor), (points[0].0, floor)]);
        self.polygon(&body, shade, 1.0);
        for pair in points.windows(2) {
            let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
            if by > ay {
                let foot = (ax + (bx - ax) * 0.3, by + (by - ay) * 0.8);
                self.polygon(&[(ax, ay), (bx, by), foot], lit, 1.0);
            }
        }
    }
}

/// A seeded generator in 0..1, the same on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// `"f6ad62"` as a colour in 0..1.
fn hex(code: &str) -> Rgb {
    let channel = |i: usize| u8::from_str_radix(&code[i..i + 2], 16).expect("a colour is six hex digits") as f32 / 255.0;
    [channel(0), channel(2), channel(4)]
}

/// Averages the samples down to the picture and writes it as an 8-bit RGB PNG.
fn write(canvas: &Canvas, path: &Path) {
    let mut rgba = Vec::with_capacity(WIDTH * HEIGHT * 4);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut sum = [0.0f32; 3];
            for sy in 0..SS {
                for sx in 0..SS {
                    let p = canvas.px[(y * SS + sy) * Canvas::W + x * SS + sx];
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += v;
                    }
                }
            }
            let n = (SS * SS) as f32;
            rgba.extend(sum.map(|s| (s / n * 255.0).round().clamp(0.0, 255.0) as u8));
            rgba.push(255);
        }
    }
    let size = Extent3d { width: WIDTH as u32, height: HEIGHT as u32, depth_or_array_layers: 1 };
    let image = Image::new(size, TextureDimension::D2, rgba, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    let picture = image.try_into_dynamic().expect("an RGBA8 image converts").to_rgb8();
    std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("the folder can be made");
    picture.save(path).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    let bytes = std::fs::metadata(path).map_or(0, |m| m.len());
    println!("{} ({WIDTH}×{HEIGHT}, {} KB)", path.display(), bytes / 1024);
}
