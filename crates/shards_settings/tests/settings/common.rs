//! The engine as a game has it, with a settings folder of the test's own.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};
use shards::ai_core::{call_tool, AiJournal, CallOutcome, JournalKind};
use shards::core::{EngineConfig, SettingsStore};
use shards::prelude::*;
use shards::testing;

/// An asset folder with a `content` folder and nothing in it: the engine's own content, no game's.
pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/settings/assets");

/// A folder of its own under the temp folder, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("shards_settings_{tag}_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `settings.toml` in the folder.
    pub fn file(&self) -> PathBuf {
        self.0.join("settings.toml")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The whole engine, headless and paused, with no settings store (the defaults, nothing on disk):
/// a test, `--no-settings`, `--verify`.
pub fn game() -> App {
    game_with(SettingsStore::Off, |_| {})
}

/// The engine with its settings kept in `dir`, as `shards::run` does for `--settings-dir`.
pub fn game_in(dir: &TempDir) -> App {
    game_with(SettingsStore::Dir(dir.path().display().to_string()), |_| {})
}

/// The engine with `store`, after `setup` (a game registering its own groups) and the way
/// `shards::run` starts the store: once the game's builder is done.
pub fn game_with(store: SettingsStore, setup: impl FnOnce(&mut App)) -> App {
    testing::test_app(
        |config| {
            config.settings = store;
            config.asset_dir = ASSETS.into();
        },
        |app| {
            setup(app);
            let config = app.world().resource::<EngineConfig>().clone();
            shards::settings::install(app, &config).unwrap();
        },
    )
}

pub fn act(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::ok(app, tool, args);
    match out.result {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

pub fn fails(app: &mut App, tool: &str, args: Value) -> String {
    let out = act(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, answered: {}", out.result);
    out.result.as_str().unwrap_or_default().to_owned()
}

/// `settings_set` that must work.
pub fn set(app: &mut App, group: &str, key: &str, value: &str) -> String {
    ok(app, "settings_set", json!({"group": group, "key": key, "value": value}))
}

/// Journal lines of one kind containing `needle`.
pub fn lines(app: &App, kind: JournalKind, needle: &str) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == kind && e.text.contains(needle)).map(|e| e.text.clone()).collect()
}
