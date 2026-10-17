//! The engine as a game has it, with the screens of `tests/ui/assets` and a hero to look at.

#![allow(dead_code)]

use serde_json::{json, Value};
use shards::ai_core::session::SessionChange;
use shards::ai_core::{call_tool, configure_session, AiJournal, CallOutcome, Scope};
use shards::prelude::*;
use shards::testing;

/// The screens of the tests, and nothing else of a game's.
pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/ui/assets");

/// The agent that plays fair: player scope, the tools a player has.
pub const PLAYER: &str = "agent";

/// The engine with the test screens, headless and paused at tick 0, the hero `e1` spawned.
pub fn game() -> App {
    game_with(|_| {})
}

/// [`game`] after `setup` (a test's own game actions or sources).
pub fn game_with(setup: impl FnOnce(&mut App)) -> App {
    let mut app = testing::test_app(
        |config| config.asset_dir = ASSETS.into(),
        |app| {
            app.add_systems(OnEnter(EngineState::InGame), spawn_hero);
            setup(app);
        },
    );
    configure_session(app.world_mut(), PLAYER, &SessionChange { scope: Some(Scope::Player), ..default() }).unwrap();
    app
}

fn spawn_hero(mut commands: Commands) {
    commands.spawn((Name::new("Hero"), AiKind::new("Hero"), AiPlayer, Health::new(100.0), Stamina::default(), Transform::default()));
}

/// A call as the dev agent.
pub fn dev(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

/// A call as the player agent.
pub fn play(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), PLAYER, tool, args)
}

/// The text of a call that must succeed, as the player.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = play(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    text(&out)
}

/// The error of a call that must fail, as the player.
pub fn refused(app: &mut App, tool: &str, args: Value) -> String {
    let out = play(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should have been refused: {}", out.result);
    text(&out)
}

pub fn text(out: &CallOutcome) -> String {
    match &out.result {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// What `ui_read` says.
pub fn read(app: &mut App) -> String {
    ok(app, "ui_read", json!({}))
}

/// What `observe` says.
pub fn observe(app: &mut App) -> String {
    ok(app, "observe", json!({}))
}

/// Journal lines that contain `needle`.
pub fn journal(app: &App, needle: &str) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).filter(|t| t.contains(needle)).collect()
}

/// Sets the hero's health (dev).
pub fn hurt_to(app: &mut App, health: f32) {
    let out = dev(app, "set", json!({"id": "e1", "component": "Health", "path": "current", "value": health}));
    assert!(out.ok, "{}", out.result);
}

pub fn hero_health(app: &mut App) -> f32 {
    let mut q = app.world_mut().query_filtered::<&Health, With<AiPlayer>>();
    q.single(app.world()).unwrap().current
}

/// A line of `ui_read` that starts with `path`, with its indentation trimmed.
pub fn line<'a>(tree: &'a str, path: &str) -> &'a str {
    tree.lines().map(str::trim_start).find(|l| l.starts_with(&format!("{path}  "))).unwrap_or_else(|| panic!("no line for {path} in:\n{tree}"))
}
