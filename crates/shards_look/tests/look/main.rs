//! Looks end to end on the whole engine: the content check, the tools, the status and the
//! developer menu; on the headless engine counted as drawn (its render plugins stay registered
//! without a GPU) the meshes each look draws: its materials, flat copies and outline hulls, and
//! what a `Look` put on, changed or taken off, a game's own material or mesh, an asset changed in
//! place and a content change restyle. `offscreen` draws them on a GPU and reads the pixels back
//! (`--ignored`). Every test writes its own asset folder and removes it after.

mod content;
mod offscreen;
mod styling;
mod tools;

use std::path::{Path, PathBuf};

use serde_json::Value;
use shards::ai_core::display::Offscreen;
use shards::ai_core::CallOutcome;
use shards::prelude::*;
use shards::testing;

/// The tests' looks: a toon one with an outline that stands for two glTF materials, a flat one
/// with a black outline, an unlit glowing one, a metal and a look named like a glTF material.
pub const LOOKS: &str = r##"{
    "stone": (color: Some("#6b6f78"), outline: Some((width: 1.5, color: "#16171c")), materials: ["Rock", "Rock.001"]),
    "hero": (color: Some("#408cff"), flat: true, outline: Some((width: 2.0))),
    "glass": (shading: Unlit, glow: Some("#7fe0ff")),
    "metal": (shading: Lit(roughness: 0.4, metallic: 1.0)),
    "Moss": (shading: Matte),
}"##;

/// A prefab drawn with `stone`.
pub const PROPS: &str = r#"{"pillar": (components: {"AiKind": ("Pillar"), "Look": ("stone")})}"#;

/// An asset folder of the tests' own, removed when it is dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    /// The folder with [`LOOKS`] and [`PROPS`].
    pub fn new(name: &str) -> Self {
        let folder = Self::empty(name);
        folder.put("content/look/test.look.ron", LOOKS);
        folder.put("content/props.prefab.ron", PROPS);
        folder
    }

    /// A folder with an empty `content`.
    pub fn empty(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_look_{name}_{}", std::process::id()));
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

/// [`game`], but it counts as drawing offscreen (an image no GPU draws into): the look systems
/// run on `update`, without a GPU.
pub fn drawn(assets: &Folder) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    testing::test_app(
        move |config| config.asset_dir = dir,
        |app| {
            app.insert_resource(Offscreen { image: Handle::default(), size: UVec2::new(320, 240) });
        },
    )
}

/// The text of a call that must succeed.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    text(&testing::ok(app, tool, args))
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// `ai/status` → `looks`.
pub fn status(app: &App) -> Value {
    shards::ai_core::status::status(app.world())["looks"].clone()
}
