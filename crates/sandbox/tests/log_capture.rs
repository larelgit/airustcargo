//! `warn!`/`error!` lines reach the journal with the tick they were logged at, even
//! inside one long `step`.
//!
//! These tests install the global logger, so they live in their own file (their own
//! process) and run one at a time: the newest app receives the captured lines.

use std::sync::Mutex;

use shards::ai_core::{call_tool, AiJournal, CallOutcome, JournalKind};
use shards::bevy::log::LogPlugin;
use shards::prelude::*;
use shards::serde_json::{json, Value};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Logs at fixed ticks: one warning at 30, a flood of 100 at 70.
fn noisy(tick: Res<SimTick>) {
    match tick.0 {
        30 => warn!("frost cracks under the boss"),
        70 => {
            for i in 0..100 {
                warn!("snowflake {i}");
            }
        }
        _ => {}
    }
}

/// Headless and paused, with the capture layer in Bevy's logger (and nothing on stderr).
fn app() -> App {
    let quiet = |_: &mut App| -> Option<shards::bevy::log::BoxedFmtLayer> {
        Some(Box::new(shards::bevy::log::tracing_subscriber::fmt::Layer::default().with_writer(std::io::sink)))
    };
    let log = LogPlugin { custom_layer: shards::ai_core::capture_log_layer, fmt_layer: quiet, ..default() };
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, log))
        .add_plugins(AiCorePlugin {
            sim: SimConfig { start_paused: true, ..default() },
            remote_port: None,
            ..default()
        })
        .add_systems(FixedUpdate, noisy.in_set(SimSet::Resolve));
    app.finish();
    app.cleanup();
    app.update();
    app
}

fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = call_tool(app.world_mut(), "dev", tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

/// Journal log lines (warnings and errors) as `(tick, text)`.
fn logged(app: &App) -> Vec<(u64, String)> {
    let journal = app.world().resource::<AiJournal>();
    journal
        .after(0)
        .0
        .filter(|e| matches!(e.kind, JournalKind::Warn | JournalKind::Error))
        .map(|e| (e.tick, e.text.clone()))
        .collect()
}

#[test]
fn a_warning_keeps_the_tick_it_was_logged_at() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = app();
    ok(&mut app, "step", json!({"ticks": 100}));
    let frost: Vec<_> = logged(&app).into_iter().filter(|(_, t)| t.contains("frost cracks")).collect();
    assert_eq!(frost.len(), 1, "{frost:?}");
    assert_eq!(frost[0].0, 30, "logged at tick 30, not at the end of the step: {frost:?}");
}

#[test]
fn step_until_journal_sees_a_warning_in_its_own_tick() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = app();
    let out = ok(&mut app, "step", json!({"ticks": 100, "until": [{"journal": "frost cracks"}]}));
    let text = out.result.as_str().unwrap();
    assert!(text.starts_with("ran 30 ticks (0 → 30), stopped: journal: 30 [warn]"), "{text}");
    assert_eq!(shards::ai_core::sim::tick(app.world()), 30);
}

#[test]
fn a_flood_is_cut_at_64_lines_per_tick_and_says_so() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = app();
    ok(&mut app, "step", json!({"ticks": 100}));
    let at_70: Vec<String> = logged(&app).into_iter().filter(|(tick, _)| *tick == 70).map(|(_, t)| t).collect();
    let flakes = at_70.iter().filter(|t| t.contains("snowflake")).count();
    assert_eq!(flakes, 64, "{at_70:#?}");
    assert_eq!(at_70.last().map(String::as_str), Some("36 log lines dropped (limit 64 per tick)"));
    let shown = app.world().resource::<AiJournal>().after(0).0.find(|e| e.text.contains("dropped")).unwrap().to_string();
    assert_eq!(shown, "70 [warn] 36 log lines dropped (limit 64 per tick)");
}

#[test]
fn an_action_returns_the_warnings_it_caused() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = app();
    app.add_ai_tool("creak", Scope::Dev, |_: In<Call<NoArgs>>| -> ToolResult {
        warn!("the floor creaks");
        Ok("ok: creaked".into())
    });
    warn!("an older line, not the action's");
    let out = ok(&mut app, "creak", json!({}));
    assert!(out.journal.iter().any(|l| l.contains("[warn]") && l.contains("the floor creaks")), "{:?}", out.journal);
    assert!(!out.journal.iter().any(|l| l.contains("an older line")), "{:?}", out.journal);
}
