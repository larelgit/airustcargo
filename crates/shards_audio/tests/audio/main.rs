//! Sound and music end to end on the whole engine: the mixer's voices, the music's layers, the
//! player's volumes, the engine's events and cues, snapshots, the tools, and the content check.
//! Every test writes its own asset folder (short tones, made here) and removes it after.

mod content;
mod mixer;
mod music;
mod subtitles;

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use shards::ai_core::{AiAppExt, AiJournal, CallOutcome, JournalKind};
use shards::audio::length::wav_bytes;
use shards::audio::{Mixer, Music, Voice};
use shards::prelude::*;
use shards::testing;

/// The sounds of the tests: a weak short one and a strong long one heard from where they
/// happen, one with two files and a limit of its own, and a menu's.
pub const SOUNDS: &str = r#"{
    "beep": (files: ["sounds/beep.wav"], priority: 50, spatial: (near: 2.0, far: 10.0)),
    "boom": (files: ["sounds/boom.wav"], priority: 200, spatial: (near: 2.0, far: 30.0)),
    "step": (files: ["sounds/beep.wav", "sounds/boom.wav"], max_instances: 2),
    "click": (files: ["sounds/beep.wav"], bus: Ui),
}"#;

/// Two tracks: `theme` calm, then with drums in a fight; `quiet` with one layer.
pub const MUSIC: &str = r#"{
    "theme": (
        stems: [
            (file: "music/calm.wav", layers: ["explore", "combat"]),
            (file: "music/drums.wav", layers: ["combat"]),
        ],
        layers: [(name: "explore"), (name: "combat", when: "fighting")],
        fade_in: 0.5,
        fade_out: 1.0,
        hold: 2.0,
    ),
    "quiet": (stems: [(file: "music/calm.wav", layers: ["explore"])], layers: [(name: "explore")]),
}"#;

/// Four voices, `theme` from the start, and sounds for three of the engine's events.
pub const MIXER: &str = r#"{"mixer": (voices: 4, music: "theme", sounds: {"healed": "beep", "died": "boom", "ui_opened": "click"})}"#;

/// The hero (`e1`, the listener, at the origin) and a foe (`e2`, 5 m ahead).
pub const HERO: &str = "e1";
pub const FOE: &str = "e2";

/// An asset folder of the tests' own, removed when it is dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_audio_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "content/audio/test.sound.ron", SOUNDS.as_bytes());
        write(&dir, "content/audio/test.music.ron", MUSIC.as_bytes());
        write(&dir, "content/audio/mixer.audio.ron", MIXER.as_bytes());
        write(&dir, "sounds/beep.wav", &tone(0.5, 880.0));
        write(&dir, "sounds/boom.wav", &tone(1.0, 110.0));
        write(&dir, "music/calm.wav", &tone(2.0, 220.0));
        write(&dir, "music/drums.wav", &tone(2.0, 55.0));
        Self(dir)
    }

    /// Writes `text` to `file` under the folder.
    pub fn put(&self, file: &str, text: &str) {
        write(&self.0, file, text.as_bytes());
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(dir: &Path, file: &str, bytes: &[u8]) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// `seconds` of a sine at `hz`, 8 kHz mono.
pub fn tone(seconds: f32, hz: f32) -> Vec<u8> {
    let rate = 8_000;
    let samples: Vec<i16> = (0..(seconds * rate as f32) as usize)
        .map(|i| (ops::sin(i as f32 / rate as f32 * hz * std::f32::consts::TAU) * 8_000.0) as i16)
        .collect();
    wav_bytes(rate, &samples)
}

/// Whether the test's game is in a fight: what `theme`'s `combat` layer waits on.
#[derive(Resource, Default)]
pub struct Fighting(pub bool);

/// Whether the test's foe is near: what `tense`'s `tension` layer waits on.
#[derive(Resource, Default)]
pub struct Near(pub bool);

/// The engine on `assets`, headless and paused at tick 0, with the hero and the foe.
pub fn game(assets: &Folder) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    testing::test_app(
        move |config| config.asset_dir = dir,
        |app| {
            app.init_resource::<Fighting>()
                .init_resource::<Near>()
                .add_ai_condition("near", |w| w.resource::<Near>().0)
                .add_ai_condition("fighting", |w| w.resource::<Fighting>().0)
                .add_systems(OnEnter(EngineState::InGame), spawn);
        },
    )
}

fn spawn(mut commands: Commands) {
    commands.spawn((Name::new("Hero"), AiKind::new("Hero"), AiPlayer, Health::new(100.0), Stamina::default(), Transform::default()));
    commands.spawn((Name::new("Foe"), AiKind::new("Foe"), Health::new(50.0), Transform::from_xyz(0.0, 0.0, -5.0)));
}

/// The entity with id `id`.
pub fn entity(app: &App, id: &str) -> Entity {
    let id: shards::ai_core::Id = serde_json::from_value(json!(id)).unwrap();
    id.entity(app.world()).unwrap()
}

/// A call as the dev agent.
pub fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    testing::call(app, tool, args)
}

/// The text of a call that must succeed.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    text(&testing::ok(app, tool, args))
}

/// The error of a call that must fail.
pub fn refused(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should have been refused: {}", out.result);
    text(&out)
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

pub fn step(app: &mut App, ticks: u64) {
    ok(app, "step", json!({"ticks": ticks}));
}

/// Ticks in `seconds` of simulation.
pub fn ticks(app: &App, seconds: f32) -> u64 {
    (seconds / shards::ai_core::sim::tick_seconds(app.world())).round() as u64
}

/// Steps `seconds` of simulation.
pub fn wait(app: &mut App, seconds: f32) {
    let n = ticks(app, seconds);
    step(app, n);
}

/// The voices playing, oldest first.
pub fn voices(app: &App) -> Vec<Voice> {
    app.world().resource::<Mixer>().voices().to_vec()
}

/// `beep #2`, oldest first.
pub fn playing(app: &App) -> Vec<String> {
    voices(app).iter().map(|v| format!("{} #{}", v.sound, v.id)).collect()
}

pub fn music(app: &App) -> Music {
    app.world().resource::<Music>().clone()
}

/// The gains of the current stems, by file.
pub fn stems(app: &App) -> Vec<(String, f32)> {
    music(app).stems.iter().map(|s| (s.file.clone(), s.gain)).collect()
}

/// Journal lines of `kind` that contain `needle`.
pub fn journal(app: &App, kind: JournalKind, needle: &str) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind == kind && e.text.contains(needle)).map(|e| e.text.clone()).collect()
}

/// Asserts `a` and `b` are within a thousandth.
#[track_caller]
pub fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-3, "{a} is not {b}");
}
