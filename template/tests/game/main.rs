//! The template's tests, one binary (Bevy links once): headless games driven through the
//! same tools an agent uses, with the engine's own test helpers (`shards::testing`).
//!
//! - `scenarios`: every file in `scenarios/` passes on a fresh game
//! - `determinism`: same seed and calls, same game; different seed, different fight
//! - `replays`: a recorded session plays back identically; checked-in replays still do
//! - `content`: content files are valid; the game's tool and condition behave
//! - `coop`: two heroes on a lossy link; the guest's trophy goes home with them and comes back
//! - `region`: the generated valley: 1000 seeds pass, the same seed the same bytes, the
//!   loading screen headless, a raider on the navmesh and its leash
//! - `audio`: the music turns to combat when the hero comes to the grunt; warnings and menus sound;
//!   the hero's swing whooshes with no model drawn
//! - `screens`: the HUD counts the trophies, the menu saves before it quits
//! - `main_menu`: a player meets the main menu, tests and replays go straight in; New game and
//!   Continue; the loading picture
//! - `manifest`: the game depends on `shards` and nothing else, and its settings folder is named
//!   after the package (a rename finds it)

mod audio;
mod content;
mod coop;
mod determinism;
mod main_menu;
mod manifest;
mod region;
mod replays;
mod screens;
mod scenarios;

use shards::ai_core::{AiJournal, JournalKind};
use shards::prelude::*;
use shards::serde_json::{json, Value};
use shards::testing::{call, ok};

/// The seed the reference scenario and replay were written for.
pub const SEED: u64 = 42;

/// Calls `tool` as the dev agent (the hero's body) and returns the text it answered.
pub fn say(app: &mut App, tool: &str, args: Value) -> String {
    let out = ok(app, tool, args);
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// Calls `tool`, which must fail; returns the error.
pub fn fails(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, got: {}", out.result);
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// Runs `ticks` ticks.
pub fn step(app: &mut App, ticks: u64) {
    ok(app, "step", json!({"ticks": ticks}));
}

/// The whole journal as text (agent calls included: they are deterministic too).
pub fn journal(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect()
}

/// Gameplay events only (hits, deaths): what a replay playback must reproduce.
pub fn story(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == JournalKind::Event).map(ToString::to_string).collect()
}

/// Every AI-visible entity bit for bit: id, position, rotation, health.
pub fn state(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &Transform, Option<&Health>)>();
    let mut lines: Vec<String> =
        q.iter(world).map(|(id, tf, hp)| format!("{id} {:?} {:?} {hp:?}", tf.translation, tf.rotation)).collect();
    lines.sort();
    lines
}

/// A short fight, played by calls: walk to the grunt, attack until it dies.
pub fn fight(app: &mut App) {
    say(app, "move_to", json!({"target": "e2", "within": 3}));
    step(app, 400);
    say(app, "attack", json!({"target": "e2", "repeat": true}));
    step(app, 900);
}
