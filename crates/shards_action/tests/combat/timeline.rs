//! The action timeline (B2a): charge by hold, turn spans and assist, miss recovery, cooldown
//! keys and shares, the press queue and the single-slot buffer, `while_held`. Content in
//! `assets/content/timeline.*.ron`.

use ai_core::observe::{observe, ObserveOptions};
use ai_core::AiId;
use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{
    can_start, extend_recovery, force_action, ActionEnd, ActionStarted, ActionState, Blocker, Cooldowns, FighterView, InputBuffer,
    RunExtras,
};
use shards_content::db::Source;
use shards_content::{ContentDb, ContentKinds};
use shards_core::Intent;

use crate::common::*;
use crate::reactions::{record, seen};

/// The engine's `input_buffer` in this content (`tuning.combat_tuning.ron`).
const TTL: f32 = 0.15;

fn arena() -> App {
    app_with(record)
}

/// The hero with `moveset`, at the origin facing north.
fn me(app: &mut App, moveset: &str) -> Entity {
    hero(app.world_mut(), moveset, Vec3::ZERO, N)
}

/// A dummy that never acts, at `at`.
fn dummy(app: &mut App, at: Vec3) -> Entity {
    fighter(app.world_mut(), "Dummy", "dummy", "monsters", at, S)
}

fn run(app: &App, e: Entity) -> shards_action::Running {
    app.world().get::<ActionState>(e).unwrap().current.clone().expect("an action runs")
}

fn extras(app: &App, e: Entity) -> Option<RunExtras> {
    app.world().get::<RunExtras>(e).copied()
}

fn queued(app: &App, e: Entity) -> Vec<(String, f32)> {
    app.world().get::<InputBuffer>(e).unwrap().queued.iter().map(|q| (q.input.clone(), q.left)).collect()
}

fn cooldown(app: &App, e: Entity, key: &str) -> f32 {
    app.world().get::<Cooldowns>(e).unwrap().left(key)
}

fn ends(app: &App, action: &str) -> Vec<ActionEnd> {
    seen(app).ended.iter().filter(|e| e.2 == action).map(|e| e.3).collect()
}

fn press_hold(app: &mut App, e: Entity, input: &str) {
    press(app, e, input);
    hold(app, e, input, true);
}

fn set_stamina(app: &mut App, e: Entity, value: f32) {
    app.world_mut().get_mut::<Stamina>(e).unwrap().current = value;
}

fn yaw_deg(dir: Vec3) -> f32 {
    ops::atan2(-dir.x, -dir.z).to_degrees()
}

fn lock(app: &mut App, e: Entity, target: Entity) {
    let id = *app.world().get::<AiId>(target).unwrap();
    app.world_mut().get_mut::<Intent>(e).unwrap().target = Some(id);
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

// --- charge by hold ------------------------------------------------------------------------

#[test]
fn charge_turns_after_the_hold() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press_hold(&mut app, hero, "heavy");
    step(&mut app, 11);
    assert_eq!(action(&app, hero), "t_heavy", "0.18 s held: not yet");
    assert_eq!(stamina(&app, hero), 80.0);
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_heavy_full", "0.2 s held: it turns");
    assert!(close(run(&app, hero).t, 0.2), "the clock goes on where it was: {}", run(&app, hero).t);
    assert_eq!(stamina(&app, hero), 50.0, "the charged one costs 30 more");
    assert_eq!(ends(&app, "t_heavy"), [ActionEnd::Branched]);
    // It runs on to the charged action's end: 0.8 s from the press.
    step(&mut app, 35);
    assert_eq!(action(&app, hero), "t_heavy_full");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "idle");
    assert_eq!(ends(&app, "t_heavy_full"), [ActionEnd::Finished]);
    assert!(extras(&app, hero).is_none(), "the extras go with the run");
}

#[test]
fn released_charge_stays() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press_hold(&mut app, hero, "heavy");
    step(&mut app, 6);
    hold(&mut app, hero, "heavy", false);
    step(&mut app, 2);
    // Held again: the hold had to last from the press.
    hold(&mut app, hero, "heavy", true);
    step(&mut app, 20);
    assert_eq!(action(&app, hero), "t_heavy");
    assert_eq!(stamina(&app, hero), 80.0);

    // A tap never charges.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "heavy");
    step(&mut app, 47);
    assert_eq!(action(&app, hero), "t_heavy");
    step(&mut app, 1);
    assert_eq!(ends(&app, "t_heavy"), [ActionEnd::Finished]);
}

#[test]
fn charge_waits_for_stamina() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    set_stamina(&mut app, hero, 30.0);
    press_hold(&mut app, hero, "heavy");
    step(&mut app, 14);
    assert_eq!(action(&app, hero), "t_heavy", "10 left, 30 needed: it waits");
    assert_eq!(stamina(&app, hero), 10.0, "a refused turn takes nothing");
    let seen = observe(app.world(), &ObserveOptions::default());
    assert!(seen.contains("charging, turns when the stamina is there"), "{seen}");
    set_stamina(&mut app, hero, 40.0);
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_heavy_full", "tried again on every tick");
    assert_eq!(stamina(&app, hero), 10.0);

    // Never enough: past the windup it stays what it was.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    set_stamina(&mut app, hero, 30.0);
    press_hold(&mut app, hero, "heavy");
    step(&mut app, 30);
    assert_eq!(action(&app, hero), "t_heavy");
    assert_eq!(extras(&app, hero).unwrap().charge_at, None, "it can no longer turn");
    set_stamina(&mut app, hero, 100.0);
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_heavy", "past `until`");
}

#[test]
fn charge_keeps_instance_and_ledger() {
    let mut app = app_with(|app| {
        record(app);
        app.init_resource::<Started>();
        app.add_observer(|e: On<ActionStarted>, mut s: ResMut<Started>| {
            s.0.push((e.event().action.clone(), e.event().input.clone()));
        });
    });
    let hero = me(&mut app, "t_fighter");
    let victim = dummy(&mut app, Vec3::new(0.0, 0.0, -2.0));
    press_hold(&mut app, hero, "heavy");
    step(&mut app, 1);
    let (instance, started) = (run(&app, hero).instance, app.world().get::<ActionState>(hero).unwrap().started);
    step(&mut app, 11);
    let now = run(&app, hero);
    assert_eq!((now.action.as_str(), now.instance), ("t_heavy_full", instance), "the same run, renamed");
    assert_eq!(app.world().get::<ActionState>(hero).unwrap().started, started, "no new start");
    assert_eq!(extras(&app, hero).unwrap().instance, instance);
    let starts = &app.world().resource::<Started>().0;
    let heavy = Some("heavy".to_owned());
    assert_eq!(starts, &[("t_heavy".to_owned(), heavy.clone()), ("t_heavy_full".to_owned(), heavy)], "the same input");
    // It hits once, with the charged damage.
    step(&mut app, 40);
    assert_eq!(hp(&app, victim), 50.0);
    assert_eq!(lines(&app, " t_heavy_full -").len(), 1, "{:#?}", journal(&app));
}

#[derive(Resource, Default)]
struct Started(Vec<(String, Option<String>)>);

#[test]
fn charge_into_removed_by_hot_reload_ends_the_run() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press_hold(&mut app, hero, "heavy");
    step(&mut app, 5);
    // A database without the charged action (a reload a check would refuse: the machine must not
    // fall over on it anyway).
    let db = app.world().resource::<ContentDb>();
    let sources: Vec<Source> = db
        .files()
        .map(|path| {
            let text = db.file_text(path).unwrap().replace("\"t_heavy_full\": (", "\"t_heavy_gone\": (");
            Source { path: path.to_owned(), text }
        })
        .collect();
    let rebuilt = ContentDb::build(app.world().resource::<ContentKinds>(), &sources);
    assert!(rebuilt.get::<shards_action::ActionDef>("t_heavy_full").is_none());
    app.world_mut().insert_resource(rebuilt);
    step(&mut app, 7);
    assert_eq!(action(&app, hero), "idle");
    assert_eq!(ends(&app, "t_heavy"), [ActionEnd::Removed]);
    assert!(extras(&app, hero).is_none());
}

// --- turning -------------------------------------------------------------------------------

#[test]
fn turn_rate_limits_degrees_per_tick() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let east = dummy(&mut app, Vec3::new(5.0, 0.0, 0.0));
    lock(&mut app, hero, east);
    press(&mut app, hero, "technique.5");
    step(&mut app, 1);
    // It starts where the body faced (north, yaw 0) and turns 90 degrees a second toward the east (-90).
    assert!(close(yaw_deg(run(&app, hero).facing), -1.5), "{}", yaw_deg(run(&app, hero).facing));
    step(&mut app, 9);
    assert!(close(yaw_deg(run(&app, hero).facing), -15.0), "{}", yaw_deg(run(&app, hero).facing));
    // The windup is 0.5 s: 45 degrees in all, then the facing holds.
    step(&mut app, 30);
    assert!(close(yaw_deg(run(&app, hero).facing), -45.0), "{}", yaw_deg(run(&app, hero).facing));
}

#[test]
fn assist_clamps_to_the_start_facing() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let target = dummy(&mut app, Vec3::new(0.0, 0.0, -5.0));
    lock(&mut app, hero, target);
    press(&mut app, hero, "technique.6");
    step(&mut app, 2);
    assert!(close(yaw_deg(run(&app, hero).facing), 0.0));
    // The target steps far to the east: the attack follows it only 30 degrees off where it started.
    app.world_mut().get_mut::<Transform>(target).unwrap().translation = Vec3::new(5.0, 0.0, -0.5);
    step(&mut app, 2);
    assert!(close(yaw_deg(run(&app, hero).facing), -30.0), "{}", yaw_deg(run(&app, hero).facing));
    // Back in the cone, it tracks it exactly.
    app.world_mut().get_mut::<Transform>(target).unwrap().translation = Vec3::new(1.0, 0.0, -5.0);
    step(&mut app, 1);
    let want = yaw_deg(Vec3::new(1.0, 0.0, -5.0).normalize());
    assert!(close(yaw_deg(run(&app, hero).facing), want), "{} vs {want}", yaw_deg(run(&app, hero).facing));
}

#[test]
fn no_turn_keeps_track_until() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let target = dummy(&mut app, Vec3::new(5.0, 0.0, 0.0));
    lock(&mut app, hero, target);
    press(&mut app, hero, "technique.7");
    step(&mut app, 1);
    assert!(extras(&app, hero).is_none(), "an action with none of it has no extras");
    assert!(close(yaw_deg(run(&app, hero).facing), -90.0), "it snaps, as before");
    step(&mut app, 30);
    app.world_mut().get_mut::<Transform>(target).unwrap().translation = Vec3::new(0.0, 0.0, 5.0);
    step(&mut app, 1);
    assert!(close(yaw_deg(run(&app, hero).facing), -90.0), "past `track_until` it no longer turns");
}

// --- miss recovery -------------------------------------------------------------------------

#[test]
fn miss_recovery_extends_only_on_a_whiff() {
    // Nobody there: 0.5 s + 0.6 s, ending on tick ceil(1.1 * 60) = 66.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "technique.3");
    step(&mut app, 18);
    let e = id(&app, hero);
    assert_eq!(lines(&app, "missed"), [format!("18 {e} t_lunge missed: +0.60s recovery")]);
    assert!(close(run(&app, hero).length, 1.1));
    let seen = observe(app.world(), &ObserveOptions::default());
    assert!(seen.contains("missed +0.60s"), "{seen}");
    step(&mut app, 47);
    assert_eq!(action(&app, hero), "t_lunge");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "idle");

    // A hit: 30 ticks, as authored.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    dummy(&mut app, Vec3::new(0.0, 0.0, -2.0));
    press(&mut app, hero, "technique.3");
    step(&mut app, 29);
    assert_eq!(action(&app, hero), "t_lunge");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "idle");
    assert!(lines(&app, "missed").is_empty());
}

#[test]
fn a_blocked_hit_counts_as_landed() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let guard = fighter(app.world_mut(), "Guard", "r_plain", "monsters", Vec3::new(0.0, 0.0, -2.0), S);
    hold(&mut app, guard, "block", true);
    step(&mut app, 20);
    assert_eq!(action(&app, guard), "r_guard");
    press(&mut app, hero, "technique.3");
    step(&mut app, 30);
    assert!(lines(&app, "missed").is_empty(), "{:#?}", journal(&app));
    assert!(lines(&app, "blocked").len() + lines(&app, "parried").len() == 1, "{:#?}", journal(&app));
    assert_eq!(action(&app, hero), "idle");
}

#[test]
fn miss_check_runs_before_the_tick_of_its_sweep() {
    // The window is (0.2, 0.3]: the check comes at 0.3 s, before that tick's sweep.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let victim = dummy(&mut app, Vec3::new(0.0, 0.0, -20.0));
    press(&mut app, hero, "technique.3");
    step(&mut app, 17);
    app.world_mut().get_mut::<Transform>(victim).unwrap().translation = Vec3::new(0.0, 0.0, -2.0);
    step(&mut app, 1);
    assert_eq!(hp(&app, victim), 90.0, "the last tick of the window hits");
    assert_eq!(lines(&app, "missed").len(), 1, "but it came after the check: {:#?}", journal(&app));

    // A tick earlier, it counts.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let victim = dummy(&mut app, Vec3::new(0.0, 0.0, -20.0));
    press(&mut app, hero, "technique.3");
    step(&mut app, 16);
    app.world_mut().get_mut::<Transform>(victim).unwrap().translation = Vec3::new(0.0, 0.0, -2.0);
    step(&mut app, 2);
    assert_eq!(hp(&app, victim), 90.0);
    assert!(lines(&app, "missed").is_empty(), "{:#?}", journal(&app));
}

#[test]
fn a_hit_on_the_start_tick_counts_as_landed() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let victim = dummy(&mut app, Vec3::new(0.0, 0.0, -2.0));
    press(&mut app, hero, "technique.8");
    step(&mut app, 1);
    assert_eq!(hp(&app, victim), 95.0, "the window opens on the first tick");
    assert!(extras(&app, hero).unwrap().landed, "the extras are in before the hits resolve");
    step(&mut app, 17);
    assert!(lines(&app, "missed").is_empty(), "{:#?}", journal(&app));
    assert_eq!(action(&app, hero), "idle");
}

#[test]
fn extend_recovery_lengthens_the_run() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "technique.7");
    step(&mut app, 5);
    assert!(!extend_recovery(app.world_mut(), hero, -1.0), "a negative bonus is refused");
    assert!(extend_recovery(app.world_mut(), hero, 0.5));
    assert!(close(run(&app, hero).length, 1.2));
    // 0.7 s + 0.5 s: tick 72.
    step(&mut app, 66);
    assert_eq!(action(&app, hero), "t_track");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "idle");
    assert!(extras(&app, hero).is_none());
    assert!(!extend_recovery(app.world_mut(), hero, 0.5), "nothing runs");
}

// --- cooldown keys and shares ------------------------------------------------------------

#[test]
fn cooldown_key_is_shared() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    dummy(&mut app, Vec3::new(0.0, 0.0, -2.0));
    press(&mut app, hero, "technique.3");
    step(&mut app, 31);
    assert_eq!(action(&app, hero), "idle");
    assert!(cooldown(&app, hero, "lunge") > 2.0 && cooldown(&app, hero, "t_lunge") == 0.0, "on its key");
    let view = FighterView::of(app.world(), hero).unwrap();
    let err = can_start(&view, "technique.4", DT).unwrap_err();
    assert!(matches!(&err, Blocker::Cooldown { action, .. } if action == "lunge"), "{err:?}");
    let refused = fails(&mut app, "use_technique", json!({"slot": 4}));
    assert!(refused.contains("lunge is on cooldown"), "{refused}");
}

#[test]
fn interrupted_windup_keeps_its_share() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "technique.3");
    step(&mut app, 5);
    assert!(force_action(app.world_mut(), hero, "t_stagger"));
    assert!(close(cooldown(&app, hero, "lunge"), 1.2), "40% of 3 s: {}", cooldown(&app, hero, "lunge"));
    assert_eq!(ends(&app, "t_lunge"), [ActionEnd::Interrupted]);
    assert!(extras(&app, hero).is_none());

    // Past the windup the whole cooldown stays.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "technique.3");
    step(&mut app, 14);
    assert!(force_action(app.world_mut(), hero, "t_stagger"));
    assert!(close(cooldown(&app, hero, "lunge"), 3.0 - 14.0 * DT), "{}", cooldown(&app, hero, "lunge"));
}

#[test]
fn startup_dodge_cancel_keeps_the_cooldown_share() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "technique.3");
    step(&mut app, 5);
    press(&mut app, hero, "dodge");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_roll");
    assert_eq!(ends(&app, "t_lunge"), [ActionEnd::Cancelled]);
    assert!(close(cooldown(&app, hero, "lunge"), 1.2 - DT), "{}", cooldown(&app, hero, "lunge"));
}

#[test]
fn combo_restart_keeps_the_full_cooldown() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "light");
    step(&mut app, 5);
    press(&mut app, hero, "technique.1");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_chain_2", "a `Combo` continuation");
    assert!(close(cooldown(&app, hero, "t_chain_1"), 1.0 - 6.0 * DT), "{}", cooldown(&app, hero, "t_chain_1"));

    // A dodge out of the same window keeps the share.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press(&mut app, hero, "light");
    step(&mut app, 5);
    press(&mut app, hero, "dodge");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_roll");
    assert!(close(cooldown(&app, hero, "t_chain_1"), 0.4 - DT), "{}", cooldown(&app, hero, "t_chain_1"));
}

// --- the press queue -----------------------------------------------------------------------

/// Starts `input` and steps until the run's clock is `t`; returns the run's clock.
fn run_until(app: &mut App, e: Entity, input: &str, ticks: u64) -> f32 {
    press(app, e, input);
    step(app, ticks);
    run(app, e).t
}

/// Presses `input` and steps once; the press's time left after that tick.
fn queue(app: &mut App, e: Entity, input: &str) -> f32 {
    press(app, e, input);
    step(app, 1);
    queued(app, e).into_iter().find(|(i, _)| i == input).map(|(_, left)| left).expect("the press waits")
}

#[test]
fn queued_press_waits_for_the_window() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let t = run_until(&mut app, hero, "light", 19);
    let left = queue(&mut app, hero, "light");
    assert!(close(left, TTL + (0.5 - t) - DT), "waits for the window at 0.5 s: {left} at {t}");
    let instance = run(&app, hero).instance;
    // The window opens at 0.5 s; the tick after the one that reaches it starts the continuation.
    step(&mut app, 10);
    assert_eq!(action(&app, hero), "t_chain_1");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_chain_2");
    assert_eq!(run(&app, hero).instance, instance + 1);
    assert!(lines(&app, "rejected").is_empty(), "{:#?}", journal(&app));
}

#[test]
fn queue_is_capped_when_the_run_ends() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    run_until(&mut app, hero, "light", 19);
    let left = queue(&mut app, hero, "light");
    assert!(left > TTL);
    assert!(force_action(app.world_mut(), hero, "t_stagger"));
    assert_eq!(queued(&app, hero), [("light".to_owned(), TTL)], "it no longer waits for a window that is gone");
    step(&mut app, 10);
    assert!(queued(&app, hero).is_empty());
    assert_eq!(lines(&app, "light rejected").len(), 1, "{:#?}", journal(&app));
}

/// Godot's light chain without `queue_early`: a press in the windup or the active part lives the
/// buffer's 0.15 s; one in the recovery lives 0.15 s past the chain's opening.
#[test]
fn godot_queue_life_light_in_recovery_only() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    run_until(&mut app, hero, "light", 6);
    let left = queue(&mut app, hero, "light");
    assert!(close(left, TTL - DT), "{left}");
    step(&mut app, 12);
    assert_eq!(lines(&app, "light rejected").len(), 1, "it ran out before the recovery: {:#?}", journal(&app));

    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let t = run_until(&mut app, hero, "light", 21);
    let left = queue(&mut app, hero, "light");
    assert!(close(left, TTL + (0.5 - t) - DT), "{left} at {t}");
}

/// Godot's `queue_early`: a press from the first tick lives 0.15 s past the chain's opening.
#[test]
fn godot_queue_life_light_queued_early() {
    let mut app = arena();
    let hero = me(&mut app, "t_early");
    let t = run_until(&mut app, hero, "light", 6);
    let left = queue(&mut app, hero, "light");
    assert!(close(left, TTL + (0.5 - t) - DT), "{left} at {t}");
    step(&mut app, 30);
    assert_eq!(action(&app, hero), "t_chain_2");
}

/// Godot's light with no chain: its opening is the end of the action, and a press in the recovery
/// starts the same action again when it is over.
#[test]
fn godot_queue_life_light_without_a_chain() {
    let mut app = arena();
    let hero = me(&mut app, "t_tail");
    let t = run_until(&mut app, hero, "light", 20);
    let left = queue(&mut app, hero, "light");
    assert!(close(left, TTL + (0.6 - t) - DT), "{left} at {t}");
    let instance = run(&app, hero).instance;
    // It ends on tick 36 (0.6 s); the press starts it again on the next.
    step(&mut app, 14);
    assert_eq!((action(&app, hero), run(&app, hero).instance), ("t_tail".to_owned(), instance));
    step(&mut app, 1);
    assert_eq!(ends(&app, "t_tail"), [ActionEnd::Finished], "the window at its end never cuts it");
    assert_eq!(action(&app, hero), "idle");
    assert_eq!(queued(&app, hero).len(), 1, "the press still waits");
    step(&mut app, 1);
    assert!(queued(&app, hero).is_empty());
    assert_eq!(run(&app, hero).instance, instance + 1);
}

/// Godot's technique: a press of a technique at any time lives 0.15 s past the moment a technique
/// may cut it (`technique_cancel_time`); with no such moment, the buffer's 0.15 s.
#[test]
fn godot_queue_life_technique() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let t = run_until(&mut app, hero, "technique.1", 6);
    let left = queue(&mut app, hero, "technique.3");
    assert!(close(left, TTL + (0.7 - t) - DT), "{left} at {t}");
    // The window opens at 0.7 s (tick 42); the tick after starts the press.
    step(&mut app, 35);
    assert_eq!(action(&app, hero), "t_tech");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_lunge", "it cut the technique at 0.7 s");
    assert_eq!(ends(&app, "t_tech"), [ActionEnd::Cancelled]);

    // Inside the window it starts at once.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    run_until(&mut app, hero, "technique.1", 45);
    press(&mut app, hero, "technique.1");
    step(&mut app, 1);
    assert_eq!(run(&app, hero).t, DT, "a new run");

    // No window at all: the buffer's time.
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    run_until(&mut app, hero, "heavy", 3);
    let left = queue(&mut app, hero, "technique.1");
    assert!(close(left, TTL - DT), "{left}");
}

// --- the single slot -----------------------------------------------------------------------

#[test]
fn single_slot_keeps_the_stronger_press() {
    let mut app = arena();
    let hero = me(&mut app, "t_slot");
    run_until(&mut app, hero, "heavy", 2);
    queue(&mut app, hero, "light");
    // Stronger: it takes the place.
    queue(&mut app, hero, "heavy");
    assert_eq!(queued(&app, hero).len(), 1);
    let e = id(&app, hero);
    assert!(lines(&app, &format!("{e} light rejected: heavy took its place")).len() == 1, "{:#?}", journal(&app));
    // Weaker: it is dropped.
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(queued(&app, hero)[0].0, "heavy");
    assert_eq!(lines(&app, "light rejected: heavy is waiting and comes first").len(), 1, "{:#?}", journal(&app));
    // The tools say the same before pressing.
    let refused = fails(&mut app, "light_attack", json!({}));
    assert!(refused.contains("heavy is waiting and comes first"), "{refused}");
    // A stronger press a tool makes says what it pushed out (near the end, where a dodge fits).
    step(&mut app, 36);
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(queued(&app, hero)[0].0, "light");
    let out = ok(&mut app, "dodge", json!({}));
    assert_eq!(out, "ok: queued, dodge starts in 0.10s (forward) (light no longer waits)");
    // The same press again keeps the longer time.
    let mut app = arena();
    let hero = me(&mut app, "t_slot");
    run_until(&mut app, hero, "heavy", 2);
    let first = queue(&mut app, hero, "light");
    let again = queue(&mut app, hero, "light");
    assert!(close(again, TTL - DT) && again > first - DT - 1e-4, "{first} then {again}");
    assert!(lines(&app, "rejected").is_empty(), "{:#?}", journal(&app));
}

#[test]
fn single_slot_drops_a_refused_press() {
    // Free to act but short of stamina: the press is tried and dropped at once.
    let mut app = arena();
    let hero = me(&mut app, "t_slot");
    set_stamina(&mut app, hero, 5.0);
    press(&mut app, hero, "heavy");
    step(&mut app, 1);
    assert!(queued(&app, hero).is_empty());
    let e = id(&app, hero);
    assert_eq!(lines(&app, "rejected").len(), 1, "{:#?}", journal(&app));
    assert!(lines(&app, &format!("1 {e} heavy rejected: ")).len() == 1, "{:#?}", journal(&app));
    let refused = fails(&mut app, "heavy_attack", json!({}));
    assert!(refused.contains("(a refused press is dropped, not kept)"), "{refused}");

    // A press the running action holds back still waits.
    let mut app = arena();
    let hero = me(&mut app, "t_slot");
    run_until(&mut app, hero, "heavy", 2);
    queue(&mut app, hero, "light");
    assert_eq!(queued(&app, hero).len(), 1);

    // Without `consume_on_refusal` it waits out its time.
    let mut app = arena();
    let hero = me(&mut app, "t_slot_keep");
    set_stamina(&mut app, hero, 5.0);
    let left = queue(&mut app, hero, "heavy");
    assert!(close(left, TTL - DT));
    step(&mut app, 9);
    assert_eq!(lines(&app, "heavy rejected").len(), 1, "{:#?}", journal(&app));
}

// --- while_held and the tools --------------------------------------------------------------

#[test]
fn while_held_ends_on_release() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    press_hold(&mut app, hero, "technique.2");
    step(&mut app, 30);
    assert_eq!(action(&app, hero), "t_beam");
    hold(&mut app, hero, "technique.2", false);
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "idle");
    assert_eq!(ends(&app, "t_beam"), [ActionEnd::Released]);
}

#[test]
fn tools_press_and_hold() {
    let mut app = arena();
    let hero = me(&mut app, "t_fighter");
    let out = ok(&mut app, "heavy_attack", json!({"hold": true}));
    assert_eq!(out, "ok: t_heavy (holding)");
    assert!(app.world().get::<Intent>(hero).unwrap().is_held("heavy"));
    step(&mut app, 3);
    let seen = observe(app.world(), &ObserveOptions::default());
    assert!(seen.contains("charging, turns in 0.15s"), "{seen}");
    let out = ok(&mut app, "step", json!({"ticks": 5, "until": [{"named": "charging"}]}));
    assert!(out.contains("charging"), "the `charging` condition: {out}");
    let out = ok(&mut app, "heavy_attack", json!({"hold": false}));
    assert_eq!(out, "ok: heavy released");
    assert!(!app.world().get::<Intent>(hero).unwrap().is_held("heavy"));
    step(&mut app, 20);
    assert_eq!(action(&app, hero), "t_heavy");
    let out = ok(&mut app, "step", json!({"ticks": 5, "until": [{"named": "charging"}]}));
    assert!(!out.contains("charging"), "{out}");
}
