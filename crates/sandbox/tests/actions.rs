//! `dodge` and `attack` say why they cannot happen instead of answering "ok" for an
//! action the game then silently drops. One-shot inputs wait in an input buffer for
//! up to 0.2 s; one that runs out is journaled as rejected.

use ai_core::{call_tool, AiJournal, CallOutcome, JournalKind};
use bevy::prelude::*;
use sandbox::gameplay::combat::{Action, InputBuffer, Stamina};
use sandbox::gameplay::intent::Intent;
use sandbox::gameplay::Player;
use serde_json::{json, Value};

const PLAYER: &str = "e5";

fn act(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = act(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

fn text(out: &CallOutcome) -> &str {
    out.result.as_str().unwrap_or_default()
}

fn player<C: Component>(app: &mut App) -> &C {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&C, With<Player>>();
    q.single(world).unwrap()
}

fn set(app: &mut App, component: &str, path: Option<&str>, value: Value) {
    ok(app, "set", json!({"id": PLAYER, "component": component, "path": path, "value": value}));
}

/// Ticks until the player's action matches, at most `limit`.
fn ticks_until(app: &mut App, limit: u32, pred: impl Fn(&Action) -> bool) -> Option<u32> {
    for n in 1..=limit {
        ok(app, "step", json!({"ticks": 1}));
        if pred(player::<Action>(app)) {
            return Some(n);
        }
    }
    None
}

fn journal(app: &App) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind == JournalKind::Event).map(ToString::to_string).collect()
}

#[test]
fn a_dodge_without_stamina_is_refused_and_changes_nothing() {
    let mut app = sandbox::test_app(42);
    set(&mut app, "Stamina", Some("current"), json!(10));
    ok(&mut app, "move", json!({"dir": "N", "meters": 2}));
    let refused = act(&mut app, "dodge", json!({"dir": "S"}));
    assert!(!refused.ok);
    assert_eq!(text(&refused), "not enough stamina: 10/25 (ready in 1.0s)");

    set(&mut app, "Stamina", Some("current"), json!(0));
    let refused = act(&mut app, "dodge", json!({}));
    assert!(!refused.ok && text(&refused).contains("stamina"), "{}", refused.result);
    let intent = player::<Intent>(&mut app).clone();
    assert_eq!(intent.dodge, None, "a refused call leaves the intent alone");
    assert_eq!(intent.orders.len(), 1, "and the orders: {intent:?}");
    ok(&mut app, "step", json!({"ticks": 1}));
    assert_eq!(*player::<Action>(&mut app), Action::Idle);
    assert_eq!(*player::<InputBuffer>(&mut app), InputBuffer::default());
    assert!(!journal(&app).iter().any(|l| l.contains("rejected")), "{:?}", journal(&app));
}

#[test]
fn a_windup_cannot_be_dodged_out_of() {
    let mut app = sandbox::test_app(42);
    set(&mut app, "Action", None, json!({"Windup": {"attack": "Sword", "left": 0.8}}));
    let refused = act(&mut app, "dodge", json!({"dir": "S"}));
    assert!(!refused.ok);
    assert_eq!(text(&refused), "busy: windup Sword 0.80s (ready in 0.80s); a windup cannot be cancelled");
    let refused = act(&mut app, "attack", json!({}));
    assert!(text(&refused).starts_with("busy: windup Sword 0.80s (ready in 1.05s)"), "an attack also waits for the recovery: {}", refused.result);
}

#[test]
fn an_action_possible_within_the_buffer_is_queued() {
    // Stamina comes back in 0.1 s (15 per second, 1.5 short).
    let mut app = sandbox::test_app(42);
    set(&mut app, "Stamina", Some("current"), json!(23.5));
    let queued = ok(&mut app, "dodge", json!({"dir": "S"}));
    assert_eq!(text(&queued), "ok: queued, dodges in 0.10s (S)");
    let started = ticks_until(&mut app, 12, |a| matches!(a, Action::Dodge { .. }));
    assert!(started.is_some_and(|n| n <= 12), "the dodge must start within 12 ticks: {started:?}");

    // A windup that ends in 0.1 s: the dodge cancels the recovery right after it.
    let mut app = sandbox::test_app(42);
    set(&mut app, "Action", None, json!({"Windup": {"attack": "Sword", "left": 0.1}}));
    let queued = ok(&mut app, "dodge", json!({"dir": "S"}));
    assert_eq!(text(&queued), "ok: queued, dodges in 0.10s (S)");
    assert!(ticks_until(&mut app, 12, |a| matches!(a, Action::Dodge { .. })).is_some());

    // A swing waits for the recovery to end.
    let mut app = sandbox::test_app(42);
    set(&mut app, "Action", None, json!({"Recover": {"left": 0.1}}));
    let queued = ok(&mut app, "attack", json!({}));
    assert_eq!(text(&queued), "ok: queued, swings in 0.10s");
    assert!(ticks_until(&mut app, 12, |a| matches!(a, Action::Windup { .. })).is_some());
}

#[test]
fn the_game_rules_stay_the_same() {
    // A dodge cancels a recovery: no waiting.
    let mut app = sandbox::test_app(42);
    set(&mut app, "Action", None, json!({"Recover": {"left": 0.5}}));
    assert_eq!(text(&ok(&mut app, "dodge", json!({"dir": "S"}))), "ok: dodging S");
    assert_eq!(ticks_until(&mut app, 1, |a| matches!(a, Action::Dodge { .. })), Some(1));
    // An attack does not: it is refused for a recovery longer than the buffer.
    let mut app = sandbox::test_app(42);
    set(&mut app, "Action", None, json!({"Recover": {"left": 0.5}}));
    let refused = act(&mut app, "attack", json!({}));
    assert_eq!(text(&refused), "busy: recover 0.50s (ready in 0.50s)");
    // From idle, both go at once, as before.
    let mut app = sandbox::test_app(42);
    assert_eq!(text(&ok(&mut app, "attack", json!({}))), "ok: swinging");
    assert_eq!(ticks_until(&mut app, 1, |a| matches!(a, Action::Windup { .. })), Some(1));
}

#[test]
fn an_input_that_runs_out_is_journaled_as_rejected() {
    let mut app = sandbox::test_app(42);
    set(&mut app, "Stamina", Some("current"), json!(0));
    // Straight into the intent, the way the keyboard writes it: no tool to refuse it.
    {
        let world = app.world_mut();
        let mut q = world.query_filtered::<&mut Intent, With<Player>>();
        q.single_mut(world).unwrap().dodge = Some(Vec3::Z);
    }
    ok(&mut app, "step", json!({"ticks": 6}));
    assert!(player::<InputBuffer>(&mut app).dodge.is_some(), "it waits in the buffer");
    assert!(!journal(&app).iter().any(|l| l.contains("rejected")));
    let out = ok(&mut app, "step", json!({"ticks": 7}));
    let rejected: Vec<&String> = out.journal.iter().filter(|l| l.contains("e5 dodge rejected: stamina")).collect();
    assert_eq!(rejected.len(), 1, "{:?}", out.journal);
    assert!(rejected[0].ends_with("e5 dodge rejected: stamina 3/25"), "{rejected:?}");
    assert_eq!(*player::<Action>(&mut app), Action::Idle);
    assert_eq!(player::<InputBuffer>(&mut app).dodge, None);
    // The stamina it never paid for is still there.
    assert!((player::<Stamina>(&mut app).current - 13.0 * 0.25).abs() < 1e-3);
}
