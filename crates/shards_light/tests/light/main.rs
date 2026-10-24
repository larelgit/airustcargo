//! Lights from content end to end on the whole engine: the content check, the flicker on the simulation's clock, the
//! shadow budget and the `lights` query headless, and the Bevy lights of a picture. Every test writes its own asset
//! folder and removes it after.

mod content;
mod drawn;
mod headless;

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use shards::ai_core::display::Offscreen;
use shards::ai_core::{AiCamera, CallOutcome};
use shards::light::ContentLight;
use shards::prelude::*;
use shards::testing;

/// Four lights: a steady one with a shadow, one that blinks (`a` dark, `m` as written, twice a second), one that
/// wanders and a spot with a shadow.
pub const LIGHTS: &str = r##"{
    "steady": (color: "#ff8000", intensity: 1000.0, range: 10.0, shadows: true),
    "blink": (intensity: 2000.0, range: 10.0, flicker: Pattern(pattern: "ma", rate: 2.0)),
    "wander": (kelvin: 2700.0, intensity: 3000.0, range: 10.0, flicker: Noise(rate: 6.0, depth: 0.5)),
    "beam": (kind: Spot(inner_deg: 20.0, outer_deg: 60.0), intensity: 500.0, range: 8.0, radius: 0.1, shadows: true),
}"##;

/// An asset folder of the tests' own, removed when it is dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    /// The folder with the four lights.
    pub fn new(name: &str) -> Self {
        let folder = Self::empty(name);
        folder.put("content/light/test.light.ron", LIGHTS);
        folder
    }

    /// A folder with an empty `content`.
    pub fn empty(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_light_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("content")).unwrap();
        Self(dir)
    }

    /// Writes `text` to `file` under the folder.
    pub fn put(&self, file: &str, text: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
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

/// The engine on `assets`, headless and paused at tick 0: nothing is drawn.
pub fn game(assets: &Folder) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    testing::test_app(move |config| config.asset_dir = dir, |_| {})
}

/// [`game`], but it counts as drawing offscreen (an image no GPU draws into): the content lights get Bevy lights.
pub fn drawn(assets: &Folder) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    testing::test_app(
        move |config| config.asset_dir = dir,
        |app| {
            app.insert_resource(Offscreen { image: Handle::default(), size: UVec2::new(320, 240) });
        },
    )
}

/// A content light `id` standing at `at`.
pub fn light(app: &mut App, id: &str, name: &str, at: Vec3) -> Entity {
    app.world_mut().spawn((Name::new(name.to_owned()), ContentLight::new(id), Transform::from_translation(at))).id()
}

/// The camera the shadow budget measures from.
pub fn camera(app: &mut App, at: Vec3) -> Entity {
    app.world_mut().spawn((AiCamera, Transform::from_translation(at))).id()
}

/// Runs `ticks` ticks (a stepped game: the clock is paused while they run).
pub fn step(app: &mut App, ticks: u64) {
    testing::ok(app, "step", json!({ "ticks": ticks }));
}

/// The text of a call that must succeed.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    text(&testing::ok(app, tool, args))
}

/// The error of a call that must fail.
pub fn refused(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::call(app, tool, args);
    assert!(!out.ok, "{tool} should fail: {}", text(&out));
    text(&out)
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// `ai/status` → `light`.
pub fn status(app: &App) -> Value {
    shards::ai_core::status::status(app.world())["light"].clone()
}
