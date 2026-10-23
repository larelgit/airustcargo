//! Writes the sounds and music of the sandbox and the template: all of it synthesized here, so the
//! repository holds no recordings of anyone's. Run it again after changing a recipe:
//!
//! ```text
//! cargo run -p sandbox --example make_sounds
//! ```
//!
//! Mono 16-bit WAV: effects at 22 050 Hz, music at 16 000 Hz. The noise is seeded, so the files
//! come out the same every time, on every OS (`sin` and `exp` are `bevy::math::ops`, that is `libm`, not the
//! platform's math library). A track's stems are one length, and what rings past the end of
//! the loop is added back at its start, so they loop without a seam.

use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};

use shards::audio::length::wav_bytes;
use shards::bevy::math::ops::{self, FloatPow};

const SFX_RATE: u32 = 22_050;
const MUSIC_RATE: u32 = 16_000;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sandbox = root.join("assets");
    let template = root.parent().and_then(Path::parent).expect("crates/sandbox is two folders down").join("template/assets");
    effects(&sandbox, &[("warden_slash", slash), ("warden_slam", slam), ("warden_sweep", sweep), ("warden_shockwave", shockwave)]);
    effects(&template, &[("grunt_swipe", slash), ("grunt_slam", slam)]);
    ambient(&sandbox.join("sounds"));
    music(&sandbox.join("music"), "arena", &ARENA);
    music(&template.join("music"), "yard", &YARD);
}

/// A sound's file name and how to make its samples.
type Recipe = (&'static str, fn(&mut Noise) -> Vec<f32>);

/// The sounds both games have, and the attacks' own.
fn effects(assets: &Path, attacks: &[Recipe]) {
    let dir = assets.join("sounds");
    let mut noise = Noise::new(0x5EED_A0D1);
    let common: [Recipe; 13] = [
        ("hit_1", |n| hit(n, 1.0)),
        ("hit_2", |n| hit(n, 0.85)),
        ("block", |n| clang(n, 420.0, 0.45)),
        ("parry", |n| clang(n, 880.0, 0.7)),
        ("whoosh", |n| whoosh(n, 0.28, 900.0, 2600.0)),
        ("guard_break", guard_break),
        ("death", death),
        ("heal", |_| chime(&[523.25, 659.25, 783.99], 0.09, 0.5)),
        ("ui_move", |_| blip(1320.0, 0.035)),
        ("ui_press", |_| chime(&[880.0, 1318.5], 0.05, 0.08)),
        ("ui_tick", |_| blip(2200.0, 0.02)),
        ("ui_open", |_| glide(500.0, 900.0, 0.14)),
        ("ui_close", |_| glide(900.0, 500.0, 0.14)),
    ];
    for (name, make) in common.iter().chain(attacks) {
        write(&dir.join(format!("{name}.wav")), SFX_RATE, &make(&mut noise), 0.85);
    }
}

// --- effects ---------------------------------------------------------------------------------

fn len(seconds: f32, rate: u32) -> usize {
    (seconds * rate as f32).round() as usize
}

fn time(i: usize) -> f32 {
    i as f32 / SFX_RATE as f32
}

/// A blade through the air: noise through a band that rises and falls.
fn slash(n: &mut Noise) -> Vec<f32> {
    whoosh(n, 0.32, 700.0, 3200.0)
}

fn sweep(n: &mut Noise) -> Vec<f32> {
    whoosh(n, 0.6, 400.0, 1800.0)
}

fn whoosh(n: &mut Noise, seconds: f32, low: f32, high: f32) -> Vec<f32> {
    let count = len(seconds, SFX_RATE);
    let (mut lo, mut hi) = (Lowpass::default(), Lowpass::default());
    (0..count)
        .map(|i| {
            let x = i as f32 / count as f32;
            let centre = low + (high - low) * ops::sin(PI * x);
            let band = hi.run(n.next(), centre * 1.6, SFX_RATE) - lo.run(n.last, centre * 0.6, SFX_RATE);
            band * ops::sin(PI * x).squared()
        })
        .collect()
}

/// Something heavy meeting the ground: a falling tone, a burst, a rumble.
fn slam(n: &mut Noise) -> Vec<f32> {
    let count = len(0.9, SFX_RATE);
    let mut rumble = Lowpass::default();
    let mut phase = 0.0;
    (0..count)
        .map(|i| {
            let t = time(i);
            phase += TAU * 55.0 * (1.0 + 2.5 * ops::exp(-t * 25.0)) / SFX_RATE as f32;
            let tone = ops::sin(phase) * ops::exp(-t * 5.0);
            let burst = n.next() * ops::exp(-t * 40.0) * 0.6;
            let low = rumble.run(n.last, 180.0, SFX_RATE) * ops::exp(-t * 3.0) * 2.5;
            tone + burst + low
        })
        .collect()
}

/// A slam that rolls out: the slam, then a swelling rumble.
fn shockwave(n: &mut Noise) -> Vec<f32> {
    let mut out = slam(n);
    out.resize(len(1.3, SFX_RATE), 0.0);
    let mut rumble = Lowpass::default();
    for (i, s) in out.iter_mut().enumerate() {
        let t = time(i);
        let swell = (t / 0.35).min(1.0) * ops::exp(-(t - 0.35).max(0.0) * 3.5);
        *s += rumble.run(n.next(), 120.0 + 260.0 * swell, SFX_RATE) * swell * 3.0;
    }
    out
}

/// A blow landing: a thump and a slap.
fn hit(n: &mut Noise, pitch: f32) -> Vec<f32> {
    let count = len(0.18, SFX_RATE);
    let mut slap = Lowpass::default();
    (0..count)
        .map(|i| {
            let t = time(i);
            let thump = ops::sin(TAU * 95.0 * pitch * t * (1.0 + ops::exp(-t * 60.0))) * ops::exp(-t * 22.0);
            thump + slap.run(n.next(), 2400.0 * pitch, SFX_RATE) * ops::exp(-t * 45.0) * 1.4
        })
        .collect()
}

/// Metal on metal: partials that do not agree, ringing down.
fn clang(n: &mut Noise, base: f32, ring: f32) -> Vec<f32> {
    let count = len(ring + 0.1, SFX_RATE);
    let partials = [(1.0, 1.0, 6.0), (2.76, 0.6, 9.0), (5.4, 0.4, 14.0), (8.93, 0.25, 20.0)];
    (0..count)
        .map(|i| {
            let t = time(i);
            let tone: f32 = partials.iter().map(|(r, a, d)| a * ops::sin(TAU * base * r * t) * ops::exp(-t * d / ring)).sum();
            tone + n.next() * ops::exp(-t * 120.0) * 0.8
        })
        .collect()
}

/// A guard giving way: a crack, then a dull clang.
fn guard_break(n: &mut Noise) -> Vec<f32> {
    let mut out: Vec<f32> = (0..len(0.06, SFX_RATE)).map(|i| n.next() * ops::exp(-time(i) * 50.0)).collect();
    out.extend(clang(n, 260.0, 0.4).into_iter().map(|s| s * 0.9));
    out
}

/// A fall: a tone sinking under a breath of noise.
fn death(n: &mut Noise) -> Vec<f32> {
    let count = len(0.9, SFX_RATE);
    let mut breath = Lowpass::default();
    let mut phase = 0.0;
    (0..count)
        .map(|i| {
            let t = time(i);
            phase += TAU * (220.0 * (1.0 - 0.6 * t)) / SFX_RATE as f32;
            let env = (t / 0.05).min(1.0) * ops::exp(-t * 3.0);
            (ops::sin(phase) + 0.3 * ops::sin(2.0 * phase)) * env + breath.run(n.next(), 900.0, SFX_RATE) * env * 1.5
        })
        .collect()
}

/// Bell notes one after another, each ringing over the next.
fn chime(notes: &[f32], gap: f32, ring: f32) -> Vec<f32> {
    let count = len(gap * notes.len() as f32 + ring, SFX_RATE);
    (0..count)
        .map(|i| {
            let t = time(i);
            notes
                .iter()
                .enumerate()
                .map(|(k, f)| {
                    let at = t - gap * k as f32;
                    if at < 0.0 { 0.0 } else { (ops::sin(TAU * f * at) + 0.3 * ops::sin(TAU * 2.0 * f * at)) * ops::exp(-at * 5.0 / ring) }
                })
                .sum()
        })
        .collect()
}

/// A short tick.
fn blip(freq: f32, seconds: f32) -> Vec<f32> {
    let count = len(seconds, SFX_RATE);
    (0..count).map(|i| ops::sin(TAU * freq * time(i)) * (1.0 - i as f32 / count as f32).squared()).collect()
}

/// A tone sliding from one pitch to another.
fn glide(from: f32, to: f32, seconds: f32) -> Vec<f32> {
    let count = len(seconds, SFX_RATE);
    let mut phase = 0.0;
    (0..count)
        .map(|i| {
            let x = i as f32 / count as f32;
            phase += TAU * (from + (to - from) * x) / SFX_RATE as f32;
            ops::sin(phase) * ops::sin(PI * x)
        })
        .collect()
}

// --- placed sounds -----------------------------------------------------------------------

/// The sandbox's placed sounds (a looped sound plays from an emitter): `room_hum`, a mains hum. It is a loop,
/// so every partial and the slow swell make whole cycles in its length and the end meets the start with
/// no click; there is no noise in it for the same reason.
fn ambient(dir: &Path) {
    let count = len(2.0, SFX_RATE);
    let partials = [(50.0, 1.0), (100.0, 0.6), (150.0, 0.35), (250.0, 0.12)];
    let hum: Vec<f32> = (0..count)
        .map(|i| {
            let t = time(i);
            let swell = 0.85 + 0.15 * ops::sin(TAU * 0.5 * t);
            partials.iter().map(|(f, a)| a * ops::sin(TAU * f * t)).sum::<f32>() * swell
        })
        .collect();
    write(&dir.join("room_hum.wav"), SFX_RATE, &hum, 0.5);
}

// --- music -----------------------------------------------------------------------------------

/// A track: its tempo, its chords (each two beats; four chords, eight beats, one loop) and the
/// octave its bass is in.
struct Track {
    bpm: f32,
    /// Chords as MIDI notes, root first.
    chords: [[u8; 3]; 4],
}

/// The sandbox's arena: A minor, 80 bpm (Am F C G), a loop of six seconds.
const ARENA: Track = Track { bpm: 80.0, chords: [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]] };

/// The template's yard: D minor, 90 bpm (Dm B♭ F C).
const YARD: Track = Track { bpm: 90.0, chords: [[50, 53, 57], [46, 50, 53], [53, 57, 60], [48, 52, 55]] };

fn hz(note: u8) -> f32 {
    440.0 * ops::powf(2f32, (f32::from(note) - 69.0) / 12.0)
}

/// Three stems of one loop: a pad (every layer), a bass pulse (tension and combat) and drums
/// (combat).
fn music(dir: &Path, name: &str, track: &Track) {
    let beat = 60.0 / track.bpm;
    let count = len(beat * 8.0, MUSIC_RATE);
    let mut noise = Noise::new(0xD0_0D1E);
    write(&dir.join(format!("{name}_pad.wav")), MUSIC_RATE, &pad(track, beat, count), 0.35);
    write(&dir.join(format!("{name}_pulse.wav")), MUSIC_RATE, &pulse(track, beat, count), 0.32);
    write(&dir.join(format!("{name}_drums.wav")), MUSIC_RATE, &drums(&mut noise, beat, count), 0.45);
}

/// Adds `voice` (seconds from its start → sample) into `loop_` from `at` for `seconds`, wrapping
/// past the end back to the start.
fn add(loop_: &mut [f32], at: f32, seconds: f32, voice: impl Fn(f32) -> f32) {
    let start = len(at, MUSIC_RATE);
    for i in 0..len(seconds, MUSIC_RATE) {
        let n = loop_.len();
        loop_[(start + i) % n] += voice(i as f32 / MUSIC_RATE as f32);
    }
}

/// Slow chords: each tone a soft triangle with a little chorus, swelling in and dying away.
fn pad(track: &Track, beat: f32, count: usize) -> Vec<f32> {
    let mut out = vec![0.0; count];
    let span = beat * 2.0;
    for (k, chord) in track.chords.iter().enumerate() {
        for &note in chord {
            let f = hz(note + 12);
            add(&mut out, span * k as f32, span + 0.6, |t| {
                let env = (t / 0.4).min(1.0) * (1.0 - ((t - span + 0.2) / 0.8).clamp(0.0, 1.0));
                (triangle(f * t) + triangle(f * 1.004 * t) + 0.5 * triangle(f * 2.0 * t)) * env * 0.4
            });
        }
    }
    out
}

/// The chord's root low, on every eighth note, plucked.
fn pulse(track: &Track, beat: f32, count: usize) -> Vec<f32> {
    let mut out = vec![0.0; count];
    let eighth = beat / 2.0;
    for step in 0..16 {
        let root = track.chords[step / 4][0];
        let f = hz(root - 12);
        let accent = if step % 2 == 0 { 1.0 } else { 0.7 };
        add(&mut out, eighth * step as f32, eighth, |t| {
            let saw: f32 = (1..=5).map(|h| ops::sin(TAU * f * h as f32 * t) / h as f32).sum();
            saw * ops::exp(-t * 9.0) * (1.0 - t / eighth).max(0.0) * accent
        });
    }
    out
}

/// Kick on one and three, snare on two and four, a hat on every eighth.
fn drums(n: &mut Noise, beat: f32, count: usize) -> Vec<f32> {
    let mut out = vec![0.0; count];
    for b in 0..8 {
        let at = beat * b as f32;
        if b % 2 == 0 {
            add(&mut out, at, 0.45, |t| ops::sin(TAU * 50.0 * t * (1.0 + 1.6 * ops::exp(-t * 18.0))) * ops::exp(-t * 7.0) * 1.2);
        } else {
            let hiss: Vec<f32> = (0..len(0.25, MUSIC_RATE)).map(|_| n.next()).collect();
            add(&mut out, at, 0.25, |t| {
                let i = ((t * MUSIC_RATE as f32) as usize).min(hiss.len() - 1);
                hiss[i] * ops::exp(-t * 16.0) * 0.7 + ops::sin(TAU * 190.0 * t) * ops::exp(-t * 25.0) * 0.5
            });
        }
    }
    for e in 0..16 {
        let mut high = Lowpass::default();
        let tick: Vec<f32> = (0..len(0.06, MUSIC_RATE)).map(|_| n.next() - high.run(n.last, 5000.0, MUSIC_RATE)).collect();
        let accent = if e % 2 == 0 { 0.35 } else { 0.22 };
        add(&mut out, beat / 2.0 * e as f32, 0.06, |t| {
            let i = ((t * MUSIC_RATE as f32) as usize).min(tick.len() - 1);
            tick[i] * ops::exp(-t * 70.0) * accent
        });
    }
    out
}

fn triangle(cycles: f32) -> f32 {
    let x = cycles.fract();
    4.0 * (x - 0.5).abs() - 1.0
}

// --- plumbing --------------------------------------------------------------------------------

/// Seeded noise (xorshift): the same files every run.
struct Noise {
    state: u32,
    /// The sample last drawn, for a second filter of the same noise.
    last: f32,
}

impl Noise {
    fn new(seed: u32) -> Self {
        Self { state: seed, last: 0.0 }
    }

    fn next(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        self.last = x as f32 / u32::MAX as f32 * 2.0 - 1.0;
        self.last
    }
}

/// A one-pole low-pass filter.
#[derive(Default)]
struct Lowpass(f32);

impl Lowpass {
    fn run(&mut self, x: f32, cutoff: f32, rate: u32) -> f32 {
        let a = 1.0 - ops::exp(-TAU * cutoff / rate as f32);
        self.0 += a * (x - self.0);
        self.0
    }
}

/// `samples` scaled to peak at `peak`, as a WAV file at `path`.
fn write(path: &Path, rate: u32, samples: &[f32], peak: f32) {
    let loudest = samples.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
    let pcm: Vec<i16> = samples.iter().map(|s| (s / loudest * peak * f32::from(i16::MAX)).round() as i16).collect();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, wav_bytes(rate, &pcm)).unwrap();
    let rms = (pcm.iter().map(|s| (f32::from(*s) / f32::from(i16::MAX)).squared()).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
    println!("{} ({:.2} s, rms {rms:.3})", path.display(), samples.len() as f32 / rate as f32);
}
