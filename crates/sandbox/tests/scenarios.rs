//! End-to-end scenarios through `ai_core::call_tool`, the entry point behind BRP and
//! `shards`: what an agent sees, does and gets back.

use ai_core::prelude::*;
use ai_core::{call_tool, AgentSessions, AiJournal, CallOutcome, JournalKind};
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use sandbox::gameplay::combat::{Action, Health};
use serde_json::{json, Value};

const PLAYER: &str = "e5";
const BOSS: &str = "e6";

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

/// Every AI-visible entity, bit-exact: position, rotation, health, action.
fn state(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &Transform, Option<&Health>, Option<&Action>)>();
    let mut lines: Vec<String> = q
        .iter(world)
        .map(|(id, tf, hp, action)| format!("{id} {:?} {:?} {hp:?} {action:?}", tf.translation, tf.rotation))
        .collect();
    lines.sort();
    lines
}

/// The gameplay journal. Agent lines are left out: they only echo the calls.
fn story(app: &App) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind != JournalKind::Agent).map(ToString::to_string).collect()
}

/// The same fight, scripted the way an agent would play it.
fn fight(app: &mut App) {
    ok(app, "attack", json!({"target": BOSS, "repeat": true}));
    ok(app, "step", json!({"ticks": 240}));
    ok(app, "dodge", json!({"away_from": BOSS}));
    ok(app, "step", json!({"ticks": 360}));
}

#[test]
fn same_seed_same_fight() {
    let (mut a, mut b) = (sandbox::test_app(42), sandbox::test_app(42));
    fight(&mut a);
    fight(&mut b);
    assert_eq!(state(&mut a), state(&mut b));
    assert_eq!(story(&a), story(&b));
    let seen = |app: &mut App| text(&ok(app, "observe", json!({}))).to_owned();
    assert_eq!(seen(&mut a), seen(&mut b));
    assert!(story(&a).iter().any(|l| l.contains("e5 -> e6 Sword")), "the player never hit the boss: {:#?}", story(&a));
}

#[test]
fn seed_changes_the_fight() {
    let (mut a, mut b) = (sandbox::test_app(42), sandbox::test_app(7));
    fight(&mut a);
    fight(&mut b);
    assert_ne!(story(&a), story(&b));
}

#[test]
fn chunked_steps_match_one_long_step() {
    let (mut a, mut b) = (sandbox::test_app(42), sandbox::test_app(42));
    for app in [&mut a, &mut b] {
        ok(app, "attack", json!({"target": BOSS, "repeat": true}));
    }
    ok(&mut a, "step", json!({"ticks": 300}));
    for _ in 0..5 {
        ok(&mut b, "step", json!({"ticks": 60}));
        // Neither queries nor real frames may move a paused simulation.
        ok(&mut b, "observe", json!({}));
        b.update();
    }
    assert_eq!(ai_core::sim::tick(b.world()), 300);
    assert_eq!(state(&mut a), state(&mut b));
    assert_eq!(story(&a), story(&b));
}

/// Builds `FixedUpdate` with ambiguity detection as a hard error.
fn order_check(app: &mut App) -> Result<(), String> {
    let strict = ScheduleBuildSettings { ambiguity_detection: LogLevel::Error, ..default() };
    app.world_mut().schedule_scope(FixedUpdate, |world, schedule| {
        schedule.set_build_settings(strict);
        schedule.initialize(world).map(|_| ()).map_err(|e| e.to_string(schedule.graph(), world))
    })
}

#[test]
fn gameplay_systems_have_a_total_order() {
    let mut app = sandbox::test_app(42);
    if let Err(e) = order_check(&mut app) {
        panic!("ambiguous FixedUpdate systems make replays diverge:\n{e}");
    }
}

#[test]
fn order_check_catches_an_unordered_system() {
    let mut app = sandbox::test_app(42);
    app.add_systems(FixedUpdate, |mut q: Query<&mut Health>| q.iter_mut().for_each(|mut h| h.current += 0.0));
    let err = order_check(&mut app).expect_err("an unordered write to Health must be reported");
    assert!(err.contains("Health"), "{err}");
}

#[test]
fn boss_phase_two_on_demand() {
    let mut app = sandbox::test_app(42);
    let set = ok(&mut app, "set", json!({"id": BOSS, "component": "Health", "path": "current", "value": 1900}));
    assert!(set.changes.iter().any(|c| c.contains("hp 4000/4000 → hp 1900/4000")), "{:?}", set.changes);

    let run = ok(&mut app, "step", json!({"until": [{"named": "boss_phase_2"}]}));
    assert!(text(&run).contains("ran 1 ticks (0 → 1), stopped: condition boss_phase_2"), "{}", text(&run));
    assert!(run.journal.iter().any(|l| l.contains("e6 phase 1->2")), "{:?}", run.journal);
    assert!(run.changes.iter().any(|c| c.contains("phase 1 → phase 2")), "{:?}", run.changes);
}

#[test]
fn errors_teach_the_fix() {
    let mut app = sandbox::test_app(42);
    let mut fail = |tool: &str, args: Value| {
        let out = act(&mut app, tool, args);
        assert!(!out.ok, "{tool} should fail");
        text(&out).to_owned()
    };

    let unknown = fail("fly", json!({}));
    assert!(unknown.contains("unknown tool `fly`; available:") && unknown.contains("move_to"), "{unknown}");

    let bad = fail("teleport", json!({"id": PLAYER}));
    assert!(bad.contains("missing field `x`") && bad.contains("Expected: teleport(id: integer|string"), "{bad}");

    let nobody = fail("inspect", json!({"id": 99}));
    assert!(nobody.contains("e99 does not exist") && nobody.contains("live: e1, e2"), "{nobody}");

    let component = fail("set", json!({"id": PLAYER, "component": "Mana", "value": 5}));
    assert!(component.contains("unknown component `Mana`; this entity has:") && component.contains("Health"), "{component}");

    let condition = fail("step", json!({"until": [{"named": "nope"}]}));
    assert!(condition.contains("unknown condition `nope`; known: boss_phase_2, player_dead"), "{condition}");

    ok(&mut app, "despawn", json!({"id": "e7"}));
    let gone = act(&mut app, "inspect", json!({"id": "e7"}));
    assert!(text(&gone).contains("e7 no longer exists") && text(&gone).contains("despawned at tick 0"), "{}", text(&gone));
}

/// Ids listed in the entity sections of an observation (YOU, SEEN, NOT IN VIEW,
/// LANDMARKS), in order of appearance.
fn listed_ids(observation: &str) -> Vec<String> {
    observation
        .lines()
        .take_while(|l| !l.starts_with("SINCE"))
        .skip(1) // header
        .flat_map(|l| l.split([' ', '·']))
        .filter(|t| t.starts_with('e') && ai_core::AiId::parse(t).is_some())
        .map(str::to_owned)
        .collect()
}

#[test]
fn ai_ids_are_managed_by_the_engine() {
    let mut app = sandbox::test_app(42);
    let refused = act(&mut app, "set", json!({"id": "e7", "component": "AiId", "value": 99}));
    assert!(!refused.ok, "set must not write AiId");
    assert_eq!(text(&refused), "AiId is managed by the engine; despawn and spawn instead");
    ok(&mut app, "inspect", json!({"id": "e7"}));
    let before = listed_ids(text(&ok(&mut app, "observe", json!({}))));

    // A raw insert over the old value (what BRP `world.insert_components` does).
    let skeleton = app.world().resource::<ai_core::AiIndex>().get(AiId(7)).unwrap();
    app.world_mut().entity_mut(skeleton).insert(AiId(99));
    let moved = ok(&mut app, "inspect", json!({"id": "e99"}));
    assert!(text(&moved).starts_with("e99 Skeleton"), "{}", text(&moved));
    let old = act(&mut app, "inspect", json!({"id": "e7"}));
    assert!(!old.ok && text(&old).contains("e7 no longer exists (Skeleton renumbered to e99 at tick 0)"), "{}", old.result);

    let after = listed_ids(text(&ok(&mut app, "observe", json!({}))));
    let expected: Vec<String> = before.iter().map(|id| if id == "e7" { "e99".into() } else { id.clone() }).collect();
    let (mut after_sorted, mut expected_sorted) = (after.clone(), expected);
    after_sorted.sort();
    expected_sorted.sort();
    assert_eq!(after_sorted, expected_sorted, "every entity once, under its new id: {after:?}");
    ok(&mut app, "step", json!({"ticks": 30}));
    ok(&mut app, "inspect", json!({"id": "e99"}));
}

/// A game component whose name happens to start with `Ai`.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct AimAssist {
    strength: f32,
}

#[test]
fn set_hints_list_game_components_named_ai() {
    let mut app = sandbox::test_app(42);
    app.register_type::<AimAssist>();
    let player = app.world().resource::<ai_core::AiIndex>().get(AiId(5)).unwrap();
    app.world_mut().entity_mut(player).insert(AimAssist { strength: 0.5 });
    let hint = act(&mut app, "set", json!({"id": PLAYER, "component": "Mana", "value": 5}));
    let hint = text(&hint);
    assert!(hint.contains("AimAssist") && hint.contains("Health"), "{hint}");
    assert!(!hint.contains("AiId") && !hint.contains("AiPlayer"), "engine markers stay hidden: {hint}");
    ok(&mut app, "set", json!({"id": PLAYER, "component": "AimAssist", "path": "strength", "value": 0.9}));
}

#[test]
fn player_scope_plays_fair() {
    let mut app = sandbox::test_app(42);
    app.world_mut().resource_mut::<AgentSessions>().configure("bot", Scope::Player, None);
    let mut bot = |tool: &str, args: Value| call_tool(app.world_mut(), "bot", tool, args);

    let teleport = bot("teleport", json!({"id": PLAYER, "x": 0, "z": 0}));
    assert_eq!(text(&teleport), "`teleport` needs dev scope; agent `bot` has player scope");
    assert!(!bot("step", json!({"ticks": 10})).ok, "a player agent must not control time");

    let tools = bot("tools", json!({}));
    assert!(text(&tools).contains("move_to(") && !text(&tools).contains("teleport("), "{}", text(&tools));

    let walk = bot("move", json!({"dir": "n", "meters": 3}));
    assert!(walk.ok, "{}", walk.result);
    ok(&mut app, "step", json!({"ticks": 60}));
    let me = call_tool(app.world_mut(), "bot", "inspect", json!({"id": PLAYER}));
    assert!(text(&me).contains("at (0.0, 9."), "{}", text(&me));
}

#[test]
fn a_panicking_tool_does_not_take_the_game_down() {
    let mut app = sandbox::test_app(42);
    app.add_ai_tool("boom", Scope::Dev, |_: In<Call<NoArgs>>| -> ToolResult { panic!("kaboom") });
    for _ in 0..2 {
        let out = act(&mut app, "boom", json!({}));
        assert!(!out.ok && text(&out).starts_with("boom panicked: kaboom"), "{}", out.result);
    }
    let errors: Vec<String> = app
        .world()
        .resource::<AiJournal>()
        .after(0)
        .0
        .filter(|e| e.kind == JournalKind::Error)
        .map(ToString::to_string)
        .collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    let schedules = app.world().resource::<Schedules>();
    assert!(schedules.contains(bevy::app::FixedMain) && schedules.contains(FixedUpdate), "schedules survive a tool panic");
    ok(&mut app, "step", json!({"ticks": 30}));
    assert_eq!(ai_core::sim::tick(app.world()), 30);
}
