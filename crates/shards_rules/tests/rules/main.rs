//! The rules end to end on the whole engine: a new world plays `start`, the main menu ends it, a
//! region trip keeps it, `rules_set` changes it (and refuses, recording nothing), content edits
//! move it on, the session is counted each tick, and snapshots put it back. Every test writes its
//! own asset folder (modes `gentle`, `plain` and `harsh`, class `brute`, charge `flask`) and removes
//! it after.

mod snapshot;
mod state;

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use shards::ai_core::{AiJournal, CallOutcome, JournalKind};
use shards::prelude::*;
use shards::rules::ActiveRules;
use shards::testing;

/// The rules: three modes, a new world at `plain`, the class `brute`, which a session of two makes tougher.
pub const RULES: &str = r#"{
    "default": (
        modes: ["gentle", "plain", "harsh"],
        start: "plain",
        classes: ["brute"],
        session: {"brute": (max_health: [1.0, 1.5])},
    ),
}"#;

pub const SETS: &str = r#"{
    "gentle": (
        name: "Gentle",
        summary: "Half the damage",
        classes: {
            "player": (incoming_damage: 0.5, perfect_bonus: 0.05, stamina_cost: [(tags: ["roll"], factor: 0.5)], charges: {"flask": 3}),
            "brute": (max_health: 0.5),
        },
    ),
    "plain": (name: "Plain", classes: {"player": (charges: {"flask": 3})}),
    "harsh": (
        name: "Harsh",
        classes: {"player": (incoming_damage: 1.5, charges: {"flask": 2}), "brute": (max_health: 1.5)},
        attack_tokens: (shift: 1),
        attack_rest: 0.5,
    ),
}"#;

/// An asset folder of the tests' own, removed when it is dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    /// The folder with the rules and the three rule sets.
    pub fn new(name: &str) -> Self {
        let folder = Self::empty(name);
        folder.put("content/rules/game.rules.ron", RULES);
        folder.put("content/rules/modes.rule_set.ron", SETS);
        folder
    }

    /// A folder with an empty `content`: a game without rules.
    pub fn empty(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_rules_{name}_{}", std::process::id()));
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

/// The engine on `assets`, headless and paused at tick 0, in a world.
pub fn game(assets: &Folder) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    testing::test_app(move |config| config.asset_dir = dir, |_| {})
}

/// The rules in play, if any.
pub fn rules(app: &App) -> Option<ActiveRules> {
    app.world().get_resource::<ActiveRules>().cloned()
}

/// `ActiveRules` with these numbers.
pub fn playing(mode: &str, generation: u32, players: u32) -> Option<ActiveRules> {
    Some(ActiveRules { mode: mode.into(), generation, players })
}

/// A player's body: `PlayerId(player)` and `hp` health.
pub fn body(app: &mut App, player: u8, hp: f32) -> Entity {
    app.world_mut()
        .spawn((Name::new(format!("player {player}")), AiKind::new("hero"), PlayerId(player), Health::new(hp), Transform::default()))
        .id()
}

/// Runs `ticks` ticks.
pub fn step(app: &mut App, ticks: u64) {
    ok(app, "step", json!({ "ticks": ticks }));
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

/// `ai/status` → `rules`.
pub fn status(app: &App) -> Value {
    shards::ai_core::status::status(app.world())["rules"].clone()
}

/// Journal lines of `kind` that contain `part`.
pub fn lines(app: &App, kind: JournalKind, part: &str) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == kind && e.text.contains(part)).map(|e| e.text.clone()).collect()
}

/// Goes to `state` as a menu or a region change would, and waits until loading is done.
pub fn go(app: &mut App, state: EngineState) {
    app.world_mut().resource_mut::<NextState<EngineState>>().set(state);
    app.update();
    testing::wait_until_loaded(app, testing::LOAD_TIMEOUT).unwrap();
}

/// The engine state now.
pub fn state(app: &App) -> EngineState {
    *app.world().resource::<State<EngineState>>().get()
}
