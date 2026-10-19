//! Context and paired actions (B3): memories, bindings that change what an input starts, the
//! fresh press a guard's perfect window needs, finishers and holds.

use ai_core::AiId;
use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{
    pair_victim, remember, ContextInputs, DamageCtx, Damaged, GuardArm, HitWeight, Memory, PairEnd, PairEnded, Paired, Pairing,
    Protection, RunHalted,
};
use shards_action::{can_start, forget, ActionAppExt, ActionDef, FighterView};
use shards_content::ContentDb;
use shards_core::{Intent, PlayerId};

use crate::common::*;

/// The hero (`c_hero`, e5 or so) at the origin facing north, a foe (`c_foe`) 1.8 m north of it
/// facing it.
fn arena() -> (App, Entity, Entity) {
    let mut app = app_with(|app| {
        app.add_context_condition("brave", |world, e| world.get::<Health>(e).is_some_and(|h| h.current > 50.0));
    });
    let world = app.world_mut();
    let hero = hero(world, "c_hero", Vec3::ZERO, N);
    let foe = spawn_foe(world, Vec3::new(0.0, 0.0, -1.8));
    step(&mut app, 2);
    (app, hero, foe)
}

fn spawn_foe(world: &mut World, at: Vec3) -> Entity {
    fighter(world, "Foe", "c_foe", "monsters", at, S)
}

/// Breaks `e`'s poise as a hit would (the dev `stagger` with `poise=true`).
fn break_poise(app: &mut App, e: Entity) {
    let who = id(app, e);
    let out = ok(app, "stagger", json!({"id": who, "poise": true}));
    assert!(out.contains("poise broken"), "{out}");
}

fn remembers(app: &App, e: Entity, key: &str) -> Option<f32> {
    app.world().get::<Memory>(e).and_then(|m| m.0.iter().find(|r| r.key == key).map(|r| r.left))
}

fn bound(app: &App, e: Entity, input: &str) -> Option<String> {
    app.world().get::<ContextInputs>(e).and_then(|c| c.bound(input)).map(|b| b.action.clone())
}

fn perfect_now(app: &App, e: Entity) -> bool {
    app.world().get::<ActionState>(e).unwrap().current.as_ref().is_some_and(|r| r.windows.perfect)
}

fn mem(app: &mut App, e: Entity, key: &str, seconds: f32) {
    remember(app.world_mut(), e, key, seconds);
}

/// The foe swings; the hero raises its guard `after` ticks later (a fresh press) and holds it.
fn parry(app: &mut App, hero: Entity, foe: Entity, after: u64) {
    press(app, foe, "light");
    step(app, after);
    hold(app, hero, "block", true);
    step(app, 20);
}

#[test]
fn memory_follows_its_events_and_forgets() {
    let (mut app, hero, foe) = arena();
    parry(&mut app, hero, foe, 6);
    assert!(!lines(&app, "parried").is_empty(), "{:#?}", journal(&app));
    let left = remembers(&app, hero, "parried").expect("a parry is remembered");
    assert!(left > 0.0 && left < 0.5, "{left}");
    // A roll starting forgets it (`Started("dodge")`); its end is remembered (`Ended("dodge")`).
    hold(&mut app, hero, "block", false);
    press(&mut app, hero, "dodge");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_roll");
    assert_eq!(remembers(&app, hero, "parried"), None, "a roll closes the counter");
    step(&mut app, 24);
    assert!(remembers(&app, hero, "dodged").is_some(), "the roll's end is remembered");
    // It ages out, and the component goes with the last memory.
    step(&mut app, 20);
    assert_eq!(remembers(&app, hero, "dodged"), None);
    step(&mut app, 70);
    assert!(app.world().get::<Memory>(hero).is_none(), "{:?}", app.world().get::<Memory>(hero));
    // Hurt forgets `dodged` and is remembered itself; a reaction forgets `parried`.
    mem(&mut app, hero, "dodged", 0.3);
    mem(&mut app, hero, "parried", 0.5);
    press(&mut app, foe, "light");
    step(&mut app, 20);
    let (h, f) = (id(&app, hero), id(&app, foe));
    assert_eq!(lines(&app, &format!("{f} -> {h} c_slash -10")).len(), 1, "{:#?}", journal(&app));
    assert_eq!(remembers(&app, hero, "dodged"), None, "hurt forgets it");
    assert_eq!(remembers(&app, hero, "parried"), None, "the reaction forgets it");
    assert!(remembers(&app, hero, "hurt").is_some());
    assert!(remembers(&app, foe, "landed").is_none(), "the foe's moveset remembers nothing");
    // The API.
    assert!(forget(app.world_mut(), hero, "hurt"));
    assert!(!forget(app.world_mut(), hero, "hurt"));
}

#[test]
fn a_moveset_that_remembers_nothing_gets_no_component() {
    let (mut app, hero, foe) = arena();
    press(&mut app, hero, "light");
    step(&mut app, 30);
    assert!(!lines(&app, "c_jab -5").is_empty(), "{:#?}", journal(&app));
    assert!(app.world().get::<Memory>(foe).is_none());
    assert!(app.world().get::<ContextInputs>(foe).is_none());
    assert!(app.world().get::<GuardArm>(foe).is_none());
    assert!(remembers(&app, hero, "landed").is_some(), "the hero's hit is remembered");
}

#[test]
fn first_binding_that_holds_wins() {
    let (mut app, hero, foe) = arena();
    assert_eq!(bound(&app, hero, "light"), None, "nothing holds: the jab");
    mem(&mut app, hero, "dodged", 0.3);
    step(&mut app, 1);
    assert_eq!(bound(&app, hero, "light").as_deref(), Some("c_dodge_cut"));
    mem(&mut app, hero, "parried", 0.5);
    step(&mut app, 1);
    assert_eq!(bound(&app, hero, "light").as_deref(), Some("c_counter"), "the counter comes before the cut");
    // Without stamina the counter does not hold: the cut again.
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 0.0;
    step(&mut app, 1);
    assert_eq!(bound(&app, hero, "light").as_deref(), Some("c_dodge_cut"));
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 100.0;
    // A broken foe in reach: the finisher before everything.
    break_poise(&mut app, foe);
    step(&mut app, 1);
    let c = app.world().get::<ContextInputs>(hero).unwrap().clone();
    assert_eq!(c.bound("light").map(|b| b.action.as_str()), Some("c_finisher"));
    assert_eq!(c.victim(), app.world().get::<AiId>(foe).copied());
    let who = id(&app, hero);
    let shown = ok(&mut app, "inspect", json!({"id": who}));
    assert!(shown.contains(&format!("light -> c_finisher on {}", id(&app, foe))), "{shown}");
    // A game condition.
    assert_eq!(bound(&app, hero, "technique.2").as_deref(), Some("c_counter"));
    app.world_mut().get_mut::<Health>(hero).unwrap().current = 40.0;
    step(&mut app, 1);
    assert_eq!(bound(&app, hero, "technique.2"), None);
}

#[test]
fn bindings_apply_in_cancel_windows() {
    // An `Input("light")` window: the bound action, not the moveset's jab.
    let (mut app, hero, _) = arena();
    press(&mut app, hero, "heavy");
    step(&mut app, 30);
    assert_eq!(action(&app, hero), "c_swing");
    mem(&mut app, hero, "dodged", 0.3);
    press(&mut app, hero, "light");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_dodge_cut");
}

#[test]
fn binding_beats_a_combo_window() {
    let (mut app, hero, _) = arena();
    press(&mut app, hero, "light");
    step(&mut app, 12);
    assert_eq!(action(&app, hero), "c_jab");
    mem(&mut app, hero, "parried", 0.5);
    press(&mut app, hero, "light");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_counter", "a bound input beats the combo's c_jab_2");
    // Without a binding the combo window is as before.
    let (mut app, hero, _) = arena();
    press(&mut app, hero, "light");
    step(&mut app, 12);
    press(&mut app, hero, "light");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_jab_2");
}

#[test]
fn held_input_start_uses_the_binding() {
    let (mut app, hero, _) = arena();
    mem(&mut app, hero, "brace", 1.0);
    hold(&mut app, hero, "block", true);
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_guard_alt");
    hold(&mut app, hero, "block", false);
    step(&mut app, 80);
    assert_eq!(action(&app, hero), "idle");
    hold(&mut app, hero, "block", true);
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_guard", "the memory is over: the moveset's guard");
}

#[test]
fn buffered_press_resolves_when_it_starts() {
    let (mut app, hero, _) = arena();
    press(&mut app, hero, "dodge");
    step(&mut app, 20);
    assert_eq!(action(&app, hero), "c_roll");
    // Pressed during the roll, when nothing binds light; it starts when the roll ends, and
    // the roll's end is remembered by then.
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(bound(&app, hero, "light"), None);
    step(&mut app, 6);
    assert_eq!(action(&app, hero), "c_dodge_cut", "{:#?}", journal(&app));
    assert_eq!(remembers(&app, hero, "dodged"), None, "the cut used the memory");
}

#[test]
fn a_binding_never_restarts_the_action_it_runs() {
    let (mut app, hero, _) = arena();
    mem(&mut app, hero, "parried", 0.5);
    step(&mut app, 1);
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "c_counter");
    // `Used` forgot it at the start; were it remembered again, the running counter would not bind.
    assert_eq!(remembers(&app, hero, "parried"), None);
    mem(&mut app, hero, "parried", 0.5);
    step(&mut app, 1);
    assert_eq!(bound(&app, hero, "light"), None, "{:?}", app.world().get::<ContextInputs>(hero));
}

fn select(app: &App) -> shards_action::PairSelect {
    app.world().resource::<ContentDb>().get::<ActionDef>("c_finisher").unwrap().pair.clone().unwrap().select
}

fn broken(app: &mut App, e: Entity) {
    app.world_mut().get_mut::<Poise>(e).unwrap().broken = 1.0;
}

fn put(app: &mut App, e: Entity, at: Vec3) {
    app.world_mut().get_mut::<Transform>(e).unwrap().translation = at;
}

#[test]
fn pair_selection_rules() {
    let (mut app, hero, a) = arena();
    let b = spawn_foe(app.world_mut(), Vec3::new(1.0, 0.0, -1.0));
    step(&mut app, 1);
    let s = select(&app);
    let victim = |app: &App| pair_victim(app.world(), hero, &s);
    assert_eq!(victim(&app), None, "nobody is broken");
    broken(&mut app, a);
    broken(&mut app, b);
    // Nearest by 3D distance: b is 1.41 m away, a 1.8 m.
    assert_eq!(victim(&app), Some(b));
    // The lock-on target wins when it qualifies.
    let a_id = *app.world().get::<AiId>(a).unwrap();
    app.world_mut().get_mut::<Intent>(hero).unwrap().target = Some(a_id);
    assert_eq!(victim(&app), Some(a), "the lock wins");
    app.world_mut().get_mut::<Intent>(hero).unwrap().target = None;
    // A tie goes to the lower AiId.
    put(&mut app, b, Vec3::new(1.8 * 0.6, 0.0, -1.8 * 0.8));
    let (low, _) = if a_id < *app.world().get::<AiId>(b).unwrap() { (a, b) } else { (b, a) };
    assert_eq!(victim(&app), Some(low), "ties: the lower id");
    put(&mut app, b, Vec3::new(5.0, 0.0, 5.0));
    // Height: 1.2 m up or down.
    put(&mut app, a, Vec3::new(0.0, 1.3, -1.8));
    assert_eq!(victim(&app), None, "too high");
    put(&mut app, a, Vec3::new(0.0, 1.1, -1.8));
    assert_eq!(victim(&app), Some(a));
    // Reach: 2 m plus the victim's body radius.
    let radius = shards_action::zone::body_radius(app.world(), a);
    put(&mut app, a, Vec3::new(0.0, 0.0, -(2.0 + radius - 0.02)));
    assert_eq!(victim(&app), Some(a), "inside reach plus radius");
    put(&mut app, a, Vec3::new(0.0, 0.0, -(2.0 + radius + 0.05)));
    assert_eq!(victim(&app), None, "beyond reach plus radius");
    // Angle: 100 degrees either side of the facing (north); behind is out.
    put(&mut app, a, Vec3::new(0.0, 0.0, 1.5));
    assert_eq!(victim(&app), None, "behind");
    put(&mut app, a, Vec3::new(1.5, 0.0, -0.1));
    assert_eq!(victim(&app), Some(a), "to the side, within 100 degrees");
    // The body's facing counts, not the stick: moving south does not turn the cone round.
    put(&mut app, a, Vec3::new(0.0, 0.0, 1.5));
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = S;
    assert_eq!(victim(&app), None, "the stick does not turn the cone");
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = Vec3::ZERO;
    // Sight: a wall between them.
    put(&mut app, a, Vec3::new(0.0, 0.0, -1.8));
    let w = wall(app.world_mut(), Vec3::new(-2.0, 0.0, -1.1), Vec3::new(2.0, 2.5, -0.9));
    step(&mut app, 1);
    broken(&mut app, a);
    put(&mut app, a, Vec3::new(0.0, 0.0, -1.8));
    put(&mut app, hero, Vec3::ZERO);
    assert_eq!(victim(&app), None, "a wall between");
    app.world_mut().despawn(w);
    step(&mut app, 1);
    broken(&mut app, a);
    put(&mut app, a, Vec3::new(0.0, 0.0, -1.8));
    put(&mut app, hero, Vec3::ZERO);
    assert_eq!(victim(&app), Some(a));
    // Slack: another player 0.3 m nearer than me or more takes it from me.
    let mate = hero_mate(app.world_mut(), Vec3::new(0.5, 0.0, -1.2));
    put(&mut app, mate, Vec3::new(0.0, 0.0, -1.0));
    assert_eq!(victim(&app), None, "the mate is 1.0 m nearer");
    put(&mut app, mate, Vec3::new(0.0, 0.0, -0.1));
    put(&mut app, hero, Vec3::new(0.0, 0.0, 0.1));
    assert_eq!(victim(&app), Some(a), "the mate is only 0.2 m nearer");
}

/// A second player of the hero's team (no tools act as it).
fn hero_mate(world: &mut World, at: Vec3) -> Entity {
    let e = fighter(world, "Mate", "c_hero", "heroes", at, N);
    world.entity_mut(e).insert(PlayerId(1));
    e
}

/// Breaks the foe, presses light: the finisher starts and takes the foe.
fn finisher(app: &mut App, hero: Entity, foe: Entity) {
    break_poise(app, foe);
    step(app, 1);
    press(app, hero, "light");
    step(app, 1);
    assert_eq!(action(app, hero), "c_finisher", "{:#?}", journal(app));
}

#[test]
fn finisher_takes_its_victim_and_knocks_it_down() {
    let (mut app, hero, foe) = arena();
    let weights = watch_weights(&mut app);
    finisher(&mut app, hero, foe);
    let (h, f) = (id(&app, hero), id(&app, foe));
    assert_eq!(lines(&app, &format!("{h} takes {f} with c_finisher")).len(), 1, "{:#?}", journal(&app));
    assert_eq!(app.world().get::<Pairing>(hero).map(|p| p.victim), app.world().get::<AiId>(foe).copied());
    assert_eq!(app.world().get::<Paired>(foe).map(|p| p.by), app.world().get::<AiId>(hero).copied());
    assert_eq!(action(&app, foe), "c_held");
    let shown = ok(&mut app, "inspect", json!({"id": f}));
    assert!(shown.contains(&format!("held by {h}")), "{shown}");
    step(&mut app, 20);
    assert_eq!(weights.lock().unwrap().as_slice(), [HitWeight::Finisher]);
    assert!(!lines(&app, "reacts knockdown").is_empty(), "{:#?}", journal(&app));
    step(&mut app, 30);
    assert!(app.world().get::<Pairing>(hero).is_none() && app.world().get::<Paired>(foe).is_none());
    assert!(!lines(&app, "lets go of").is_empty() && !lines(&app, "(finished)").is_empty(), "{:#?}", journal(&app));
}

fn watch_weights(app: &mut App) -> std::sync::Arc<std::sync::Mutex<Vec<HitWeight>>> {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = seen.clone();
    app.world_mut().add_observer(move |e: On<Damaged>| into.lock().unwrap().push(e.event().weight));
    seen
}

#[test]
fn finisher_knockdown_bypasses_the_victim_action() {
    let (mut app, hero, foe) = arena();
    finisher(&mut app, hero, foe);
    // Tick by tick to the strike: the held pose until then, the knockdown on that tick.
    let mut seen = Vec::new();
    for _ in 0..20 {
        step(&mut app, 1);
        seen.push(action(&app, foe));
        if seen.last().is_some_and(|a| a == "c_react_down") {
            break;
        }
    }
    assert_eq!(seen.last().map(String::as_str), Some("c_react_down"), "{seen:?}");
    assert!(seen[..seen.len() - 1].iter().all(|a| a == "c_held"), "{seen:?}");
    let damaged = lines(&app, "c_finisher -20");
    assert_eq!(damaged.len(), 1, "{:#?}", journal(&app));
    let tick = |l: &str| l.split_whitespace().next().unwrap().to_owned();
    let reacted = lines(&app, "reacts knockdown");
    assert_eq!(tick(&damaged[0]), tick(&reacted[0]), "on the strike's tick");
    // A forced reaction starts no immunity.
    assert!(app.world().get::<Protection>(foe).is_none_or(|p| p.reactions <= 1e-4), "{:?}", app.world().get::<Protection>(foe));
}

#[test]
fn ends_break_once_then_knockdown() {
    let (mut app, hero, foe) = arena();
    finisher(&mut app, hero, foe);
    step(&mut app, 16);
    assert!(!lines(&app, "c_finisher -20").is_empty(), "{:#?}", journal(&app));
    let p = *app.world().get::<Poise>(foe).unwrap();
    assert!(!p.is_broken() && p.current == p.max, "the break ended: {p:?}");
    let protection = app.world().get::<Protection>(foe).copied().unwrap_or_default();
    assert!(protection.poise > 2.5, "break protection: {protection:?}");
    assert!(app.world().get::<Pairing>(hero).is_some_and(|p| p.broke_once));
    assert_eq!(action(&app, foe), "c_react_down");
}

#[test]
fn hold_break_keeps_the_victim_broken() {
    let (mut app, hero, foe) = arena();
    break_poise(&mut app, foe);
    app.world_mut().get_mut::<Poise>(foe).unwrap().broken = 0.05;
    step(&mut app, 1);
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "c_finisher");
    let left = app.world().get::<Poise>(foe).unwrap().broken;
    assert!(left > 0.4, "held for the strike: {left}");
    step(&mut app, 16);
    assert!(!lines(&app, "reacts knockdown").is_empty(), "the strike still found it broken: {:#?}", journal(&app));
}

#[test]
fn finisher_hits_only_its_victim() {
    let (mut app, hero, foe) = arena();
    let other = foe_at(&mut app, Vec3::new(0.6, 0.0, -1.4));
    finisher(&mut app, hero, foe);
    step(&mut app, 30);
    assert!(!lines(&app, "c_finisher -20").is_empty());
    assert_eq!(hp(&app, other), 100.0, "the other foe in the cone is not hit");
}

fn foe_at(app: &mut App, at: Vec3) -> Entity {
    let e = spawn_foe(app.world_mut(), at);
    step(app, 1);
    e
}

#[test]
fn other_attackers_can_still_hit_a_paired_victim() {
    let (mut app, hero, foe) = arena();
    // No nearer to the foe than the hero (the finisher's slack would give it to the mate).
    let mate = hero_mate(app.world_mut(), Vec3::new(2.0, 0.0, -1.8));
    app.world_mut().entity_mut(mate).insert(Transform::from_xyz(2.0, 0.0, -1.8).looking_to(-Vec3::X, Vec3::Y));
    step(&mut app, 1);
    finisher(&mut app, hero, foe);
    press(&mut app, mate, "light");
    step(&mut app, 10);
    let hits = lines(&app, &format!("{} -> {} c_jab -5", id(&app, mate), id(&app, foe)));
    assert_eq!(hits.len(), 1, "{:#?}", journal(&app));
    assert_eq!(action(&app, foe), "c_held", "no reaction while it is held");
}

#[test]
fn exclusive_victim_is_untouchable_by_others() {
    let (mut app, hero, foe) = arena();
    let mate = hero_mate(app.world_mut(), Vec3::new(1.2, 0.0, -1.8));
    app.world_mut().entity_mut(mate).insert(Transform::from_xyz(1.2, 0.0, -1.8).looking_to(-Vec3::X, Vec3::Y));
    step(&mut app, 1);
    press(&mut app, hero, "technique.1");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_grab");
    assert!(app.world().get::<Paired>(foe).is_some_and(|p| p.exclusive));
    press(&mut app, mate, "light");
    step(&mut app, 15);
    assert!(lines(&app, &format!("{} -> {}", id(&app, mate), id(&app, foe))).is_empty(), "{:#?}", journal(&app));
    assert_eq!(hp(&app, foe), 100.0);
    // Let go, it can be hit again.
    step(&mut app, 60);
    assert!(app.world().get::<Paired>(foe).is_none());
    press(&mut app, mate, "light");
    step(&mut app, 15);
    assert_eq!(lines(&app, &format!("{} -> {} c_jab -5", id(&app, mate), id(&app, foe))).len(), 1, "{:#?}", journal(&app));
}

#[test]
fn finisher_user_is_invulnerable_for_the_whole_run() {
    let (mut app, hero, foe) = arena();
    let other = spawn_foe(app.world_mut(), Vec3::new(1.5, 0.0, 0.0));
    app.world_mut().entity_mut(other).insert(Transform::from_xyz(1.5, 0.0, 0.0).looking_to(-Vec3::X, Vec3::Y));
    step(&mut app, 1);
    finisher(&mut app, hero, foe);
    for _ in 0..3 {
        press(&mut app, other, "light");
        step(&mut app, 12);
    }
    assert_eq!(hp(&app, hero), 100.0, "{:#?}", journal(&app));
    assert!(!lines(&app, &format!("{} evaded {} c_slash", id(&app, hero), id(&app, other))).is_empty(), "{:#?}", journal(&app));
}

#[test]
fn pair_lost_on_death_stops_the_lunge() {
    let (mut app, hero, foe) = arena();
    finisher(&mut app, hero, foe);
    step(&mut app, 3);
    shards_action::kill(app.world_mut(), foe, None, None);
    step(&mut app, 1);
    assert!(!lines(&app, "(lost)").is_empty(), "{:#?}", journal(&app));
    assert!(app.world().get::<RunHalted>(hero).is_some());
    let here = feet(&app, hero);
    step(&mut app, 8);
    assert_eq!(action(&app, hero), "c_finisher", "the run goes on");
    assert!(feet(&app, hero).distance(here) < 0.02, "{here} -> {}", feet(&app, hero));
    step(&mut app, 30);
    assert!(app.world().get::<RunHalted>(hero).is_none(), "the halt goes with the run");
}

#[test]
fn break_range_cuts() {
    let (mut app, hero, foe) = arena();
    press(&mut app, hero, "technique.1");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_grab");
    let mut tf = *app.world().get::<Transform>(foe).unwrap();
    tf.translation = Vec3::new(0.0, 0.0, -6.0);
    app.world_mut().entity_mut(foe).insert(tf);
    step(&mut app, 2);
    assert!(!lines(&app, "(lost)").is_empty(), "{:#?}", journal(&app));
    assert_eq!(action(&app, hero), "idle", "the hold is cut");
    assert_ne!(action(&app, foe), "c_held", "the victim is let go");
}

#[test]
fn cancel_on_hurt_cuts() {
    let (mut app, hero, foe) = arena();
    // Behind the hero, farther than the foe: the hold takes the foe.
    let other = spawn_foe(app.world_mut(), Vec3::new(0.0, 0.0, 2.2));
    app.world_mut().entity_mut(other).insert(Transform::from_xyz(0.0, 0.0, 2.2).looking_to(N, Vec3::Y));
    step(&mut app, 1);
    press(&mut app, hero, "technique.1");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_grab");
    press(&mut app, other, "light");
    step(&mut app, 16);
    assert!(!lines(&app, &format!("{} -> {} c_slash -10", id(&app, other), id(&app, hero))).is_empty(), "{:#?}", journal(&app));
    assert!(!lines(&app, "(cut)").is_empty(), "{:#?}", journal(&app));
    assert_ne!(action(&app, hero), "c_grab");
    assert!(app.world().get::<Pairing>(hero).is_none() && app.world().get::<Paired>(foe).is_none());
}

#[test]
fn counter_weight_reaches_damaged() {
    let (mut app, hero, foe) = arena();
    let weights = watch_weights(&mut app);
    parry(&mut app, hero, foe, 6);
    assert!(remembers(&app, hero, "parried").is_some(), "{:#?}", journal(&app));
    press(&mut app, hero, "light");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_counter");
    step(&mut app, 12);
    assert_eq!(weights.lock().unwrap().last(), Some(&HitWeight::Counter), "{:#?}", journal(&app));
}

// --- the guard's fresh press -------------------------------------------------------------

#[test]
fn a_fresh_press_opens_the_perfect_window() {
    let (mut app, hero, _) = arena();
    hold(&mut app, hero, "block", true);
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "c_guard");
    assert!(perfect_now(&app, hero));
    let arm = *app.world().get::<GuardArm>(hero).unwrap();
    assert!(arm.run.is_some() && !arm.armed, "{arm:?}");
}

#[test]
fn a_press_too_soon_after_the_last_does_not_arm() {
    let (mut app, hero, _) = arena();
    hold(&mut app, hero, "block", true);
    step(&mut app, 3);
    hold(&mut app, hero, "block", false);
    step(&mut app, 1);
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "c_jab");
    // Pressed again 0.1 s after the first: mashing opens nothing.
    hold(&mut app, hero, "block", true);
    step(&mut app, 30);
    assert_eq!(action(&app, hero), "c_guard");
    assert!(!perfect_now(&app, hero), "a plain block");
    assert!(!app.world().get::<GuardArm>(hero).unwrap().armed);
}

#[test]
fn a_guard_up_late_after_its_press_is_a_plain_block() {
    let (mut app, hero, _) = arena();
    press(&mut app, hero, "heavy");
    step(&mut app, 2);
    // Pressed while the swing runs; the guard comes up 0.8 s later.
    hold(&mut app, hero, "block", true);
    step(&mut app, 1);
    assert!(app.world().get::<GuardArm>(hero).unwrap().armed, "a fresh press");
    let mut started = false;
    for _ in 0..60 {
        step(&mut app, 1);
        if action(&app, hero) == "c_guard" {
            started = true;
            break;
        }
    }
    assert!(started);
    assert!(!perfect_now(&app, hero), "the press is too old");
}

#[test]
fn a_guard_held_through_an_attack_comes_back_as_a_plain_block() {
    let (mut app, hero, _) = arena();
    hold(&mut app, hero, "block", true);
    step(&mut app, 2);
    assert!(perfect_now(&app, hero));
    // The light cuts the guard; the block stays held; the guard comes back without a window.
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "c_jab");
    let mut back = false;
    for _ in 0..40 {
        step(&mut app, 1);
        if action(&app, hero) == "c_guard" {
            back = true;
            break;
        }
    }
    assert!(back);
    assert!(!perfect_now(&app, hero));
}

#[test]
fn a_parry_makes_the_next_press_fresh() {
    let (mut app, hero, foe) = arena();
    parry(&mut app, hero, foe, 6);
    assert!(!lines(&app, &format!("{} parried {} c_slash", id(&app, hero), id(&app, foe))).is_empty(), "{:#?}", journal(&app));
    hold(&mut app, hero, "block", false);
    step(&mut app, 1);
    press(&mut app, hero, "dodge");
    step(&mut app, 2);
    // 0.1 s after the press before (and well inside the gap), but a parry came in between.
    hold(&mut app, hero, "block", true);
    step(&mut app, 1);
    assert!(app.world().get::<GuardArm>(hero).unwrap().armed, "{:?}", app.world().get::<GuardArm>(hero));
}

#[test]
fn guard_arm_goes_once_it_says_nothing() {
    let (mut app, hero, foe) = arena();
    hold(&mut app, hero, "block", true);
    step(&mut app, 2);
    hold(&mut app, hero, "block", false);
    step(&mut app, 60);
    assert!(app.world().get::<GuardArm>(hero).is_none());
    assert!(app.world().get::<GuardArm>(foe).is_none(), "a moveset without the rule never gets one");
}

#[test]
fn action_ended_of_a_pair_is_its_end() {
    let (mut app, hero, foe) = arena();
    let ends = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = ends.clone();
    app.world_mut().add_observer(move |e: On<PairEnded>| into.lock().unwrap().push(e.event().end));
    press(&mut app, hero, "technique.1");
    step(&mut app, 2);
    // A stagger cuts the hold.
    let who = id(&app, hero);
    ok(&mut app, "stagger", json!({"id": who}));
    step(&mut app, 1);
    assert_eq!(ends.lock().unwrap().as_slice(), [PairEnd::Cut]);
    assert!(app.world().get::<Paired>(foe).is_none());
}

#[test]
fn a_finisher_that_kills_its_victim_does_not_cleave_the_next_foe() {
    let (mut app, hero, foe) = arena();
    let other = foe_at(&mut app, Vec3::new(0.6, 0.0, -1.4));
    // The strike (20) kills; the pair is let go as lost and the run halts, but the run still hits
    // only the victim it took.
    app.world_mut().get_mut::<Health>(foe).unwrap().current = 10.0;
    finisher(&mut app, hero, foe);
    step(&mut app, 40);
    assert!(app.world().get::<Dead>(foe).is_some(), "{:#?}", journal(&app));
    assert!(!lines(&app, "(killed)").is_empty(), "{:#?}", journal(&app));
    assert!(lines(&app, "(lost)").is_empty(), "a victim killed by the blow is not lost: {:#?}", journal(&app));
    assert_eq!(hp(&app, other), 100.0, "the foe beside it is not hit: {:#?}", journal(&app));
}

#[test]
fn a_barrier_and_a_damage_taken_multiplier_leave_the_strike_as_it_is() {
    let (mut app, hero, foe) = arena();
    let weights = watch_weights(&mut app);
    // A barrier that swallows the strike whole and a multiplier that would triple it: the pair's
    // weight and its forced reaction come from the pair, not from what the hit did to health.
    shards_action::grant_barrier(app.world_mut(), foe, 100.0, 5.0);
    shards_action::set_damage_taken(app.world_mut(), foe, "core", 3.0, None);
    finisher(&mut app, hero, foe);
    step(&mut app, 20);
    assert_eq!(weights.lock().unwrap().as_slice(), [HitWeight::Finisher]);
    assert!(!lines(&app, "reacts knockdown").is_empty(), "{:#?}", journal(&app));
    assert_eq!(hp(&app, foe), 100.0, "the barrier took it");
}

#[test]
fn a_zero_strike_reaction_asks_for_none() {
    let (mut app, hero, foe) = arena();
    let weights = watch_weights(&mut app);
    let reactions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = reactions.clone();
    app.world_mut().add_observer(move |e: On<Damaged>| into.lock().unwrap().push(e.event().reaction));
    press(&mut app, hero, "technique.3");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "c_seize", "{:#?}", journal(&app));
    assert_eq!(action(&app, foe), "c_held");
    step(&mut app, 12);
    assert_eq!(hp(&app, foe), 95.0, "{:#?}", journal(&app));
    assert_eq!(weights.lock().unwrap().as_slice(), [HitWeight::Finisher]);
    assert_eq!(reactions.lock().unwrap().as_slice(), [None], "no reaction is planned");
    assert!(lines(&app, "reacts").is_empty(), "{:#?}", journal(&app));
    assert_eq!(action(&app, foe), "c_held", "it keeps its pose");
}

static SEEN_PAIR: std::sync::Mutex<Vec<(bool, HitWeight)>> = std::sync::Mutex::new(Vec::new());

#[test]
fn a_damage_hook_sees_that_the_hit_is_a_pair_strike() {
    fn watch(ctx: &mut DamageCtx, _: &World) {
        SEEN_PAIR.lock().unwrap().push((ctx.pair, ctx.weight));
    }
    let mut app = app_with(|app| {
        app.add_damage_hook(watch);
    });
    let hero = hero(app.world_mut(), "c_hero", Vec3::ZERO, N);
    let foe = spawn_foe(app.world_mut(), Vec3::new(0.0, 0.0, -1.8));
    step(&mut app, 2);
    SEEN_PAIR.lock().unwrap().clear();
    finisher(&mut app, hero, foe);
    step(&mut app, 20);
    assert_eq!(SEEN_PAIR.lock().unwrap().as_slice(), [(true, HitWeight::Finisher)]);
    // An ordinary jab on another foe is no pair strike.
    SEEN_PAIR.lock().unwrap().clear();
    let other = foe_at(&mut app, Vec3::new(0.0, 0.0, -1.5));
    step(&mut app, 60);
    press(&mut app, hero, "light");
    step(&mut app, 12);
    assert!(hp(&app, other) < 100.0, "{:#?}", journal(&app));
    assert!(SEEN_PAIR.lock().unwrap().iter().all(|(pair, _)| !pair), "{:?}", SEEN_PAIR.lock().unwrap());
}

#[test]
fn the_body_facing_decides_the_angle_not_the_stick() {
    let (mut app, hero, foe) = arena();
    broken(&mut app, foe);
    let s = select(&app);
    assert_eq!(pair_victim(app.world(), hero, &s), Some(foe));
    // The body looks away from the broken foe while the stick points at it: no finisher.
    app.world_mut().entity_mut(hero).insert(Transform::from_xyz(0.0, 0.0, 0.0).looking_to(S, Vec3::Y));
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = N;
    assert_eq!(pair_victim(app.world(), hero, &s), None, "facing away");
    // Running away with the stick, the body still facing the foe: the finisher stays.
    app.world_mut().entity_mut(hero).insert(Transform::from_xyz(0.0, 0.0, 0.0).looking_to(N, Vec3::Y));
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = S;
    assert_eq!(pair_victim(app.world(), hero, &s), Some(foe), "the stick does not count");
}

#[test]
fn the_remember_tool_takes_a_few_seconds_not_a_billion() {
    let (mut app, hero, _) = arena();
    let who = id(&app, hero);
    let err = fails(&mut app, "remember", json!({"id": who, "key": "parried", "seconds": 1.0e9}));
    assert!(err.contains("from 0 to"), "{err}");
    let out = ok(&mut app, "remember", json!({"id": who, "key": "parried", "seconds": 5.0}));
    assert!(out.contains("remembers parried"), "{out}");
}

// --- a binding the medium refuses does not bind (FU4) ---------------------------------------

/// A pool 5 m deep at x 80..120, with a floor: a body put in it swims in a few ticks.
fn pool(app: &mut App) -> Vec3 {
    use shards_motion::{Collider, CollisionLayers, Shape, Volume, VolumeKind};
    let world = app.world_mut();
    wall(world, Vec3::new(80.0, -6.0, -20.0), Vec3::new(120.0, -5.0, 20.0));
    world.spawn((
        Collider::new(Shape::cuboid(Vec3::new(20.0, 2.5, 20.0))).with_layers(CollisionLayers::VOLUME),
        Volume { kind: VolumeKind::Water },
        Transform::from_translation(Vec3::new(100.0, -2.5, 0.0)),
    ));
    Vec3::new(100.0, -1.1, 0.0)
}

#[test]
fn a_binding_the_medium_refuses_falls_to_the_movesets_action() {
    // On the ground a broken foe in reach binds the light to the finisher...
    let (mut app, _, foe) = arena();
    let swimmer = hero(app.world_mut(), "c_swimmer", Vec3::new(10.0, 0.0, 0.0), N);
    app.world_mut().get_mut::<Transform>(foe).unwrap().translation = Vec3::new(10.0, 0.0, -1.8);
    break_poise(&mut app, foe);
    step(&mut app, 2);
    assert_eq!(bound(&app, swimmer, "light").as_deref(), Some("c_finisher"));
    assert!(app.world().get::<ContextInputs>(swimmer).is_some_and(|c| c.victim().is_some()), "the HUD would prompt on the ground");

    // ...in water the moveset's `only_in` refuses it: nothing binds, the HUD's `you.finisher` source
    // (`ContextInputs::victim`) and the AI's `finishable by` (the same list) see none, and the light is the stroke.
    let (mut app, _, foe) = arena();
    let at = pool(&mut app);
    let swimmer = hero(app.world_mut(), "c_swimmer", at, N);
    app.world_mut().get_mut::<Transform>(foe).unwrap().translation = at + Vec3::new(0.0, 0.0, -1.8);
    break_poise(&mut app, foe);
    step(&mut app, 30);
    assert_eq!(app.world().get::<shards_motion::MotionState>(swimmer).unwrap().mode, shards_motion::MotionMode::Water);
    assert!(pair_victim_exists(&app, swimmer), "the foe is still a candidate: only the medium refuses");
    assert_eq!(bound(&app, swimmer, "light"), None, "{:?}", app.world().get::<ContextInputs>(swimmer));
    assert!(app.world().get::<ContextInputs>(swimmer).is_none_or(|c| c.victim().is_none()));
    let view = FighterView::of(app.world(), swimmer).unwrap();
    assert_eq!(can_start(&view, "light", 1.0 / 60.0).map(|s| s.action), Ok("c_stroke".to_owned()));
    press(&mut app, swimmer, "light");
    step(&mut app, 2);
    assert_eq!(action(&app, swimmer), "c_stroke");
}

/// Whether `e`'s pair `select` (the finisher's) takes some victim now, whatever the medium says.
fn pair_victim_exists(app: &App, e: Entity) -> bool {
    let db = app.world().resource::<ContentDb>();
    let select = db.get::<ActionDef>("c_finisher").and_then(|d| d.pair.as_ref()).map(|p| p.select.clone()).unwrap();
    pair_victim(app.world(), e, &select).is_some()
}

#[test]
fn a_paired_run_aims_at_its_victim_not_at_the_lock() {
    // The lock is on a foe to the east that is not broken; the victim is the broken foe to the north.
    let (mut app, hero, victim) = arena();
    let other = foe_at(&mut app, Vec3::new(2.5, 0.0, 0.0));
    let other_id = *app.world().get::<AiId>(other).unwrap();
    app.world_mut().get_mut::<Intent>(hero).unwrap().target = Some(other_id);
    finisher(&mut app, hero, victim);
    let (h, v) = (id(&app, hero), id(&app, victim));
    assert_eq!(app.world().get::<Pairing>(hero).map(|p| p.victim), app.world().get::<AiId>(victim).copied(), "the lock is not the victim");
    step(&mut app, 30);
    assert_eq!(lines(&app, &format!("{h} -> {v} c_finisher -20")).len(), 1, "the blow lands on the victim: {:#?}", journal(&app));
    assert_eq!(hp(&app, victim), 80.0);
    assert_eq!(hp(&app, other), 100.0, "the locked foe is not what it turned to");
    // The run faced the victim all along (north), not the lock (east).
    let facing = app.world().get::<Transform>(hero).unwrap().forward();
    assert!(facing.dot(Vec3::NEG_Z) > 0.9, "{facing:?}");
    // An action that is not paired still follows the lock.
    step(&mut app, 60);
    press(&mut app, hero, "heavy");
    step(&mut app, 12);
    let facing = app.world().get::<Transform>(hero).unwrap().forward();
    assert!(facing.dot(Vec3::X) > 0.9, "a plain swing turns to the lock: {facing:?}");
}

#[test]
fn a_paired_run_started_from_a_moveset_input_aims_at_the_victim_the_select_finds() {
    // `technique.1` is a hold with no `When::Pair` binding: its victim is found when the run starts (the nearest foe, to the
    // north), while the lock is on a foe to the east that is farther.
    let (mut app, hero, near) = arena();
    put(&mut app, near, Vec3::new(0.0, 0.0, -1.0));
    let other = foe_at(&mut app, Vec3::new(1.8, 0.0, 0.0));
    let other_id = *app.world().get::<AiId>(other).unwrap();
    app.world_mut().get_mut::<Intent>(hero).unwrap().target = Some(other_id);
    step(&mut app, 1);
    press(&mut app, hero, "technique.1");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "c_grab", "{:#?}", journal(&app));
    assert_eq!(app.world().get::<Pairing>(hero).map(|p| p.victim), app.world().get::<AiId>(near).copied());
    step(&mut app, 2);
    let facing = app.world().get::<Transform>(hero).unwrap().forward();
    assert!(facing.dot(Vec3::NEG_Z) > 0.9, "the run faces the victim, not the lock: {facing:?}");
}

#[test]
fn a_pairs_angle_can_be_measured_from_what_the_fighter_means() {
    let (mut app, hero, foe) = arena();
    let other = foe_at(&mut app, Vec3::new(0.0, 0.0, 5.0));
    broken(&mut app, foe);
    step(&mut app, 1);
    let facing = select(&app);
    let intent = app.world().resource::<ContentDb>().get::<ActionDef>("c_finisher_intent").unwrap().pair.clone().unwrap().select;
    assert_eq!(facing.angle_from, shards_action::PairAngle::Facing, "the default");
    assert_eq!(intent.angle_from, shards_action::PairAngle::Intent);
    let foe_id = *app.world().get::<AiId>(foe).unwrap();
    let other_id = *app.world().get::<AiId>(other).unwrap();
    let victim = |app: &App, s: &shards_action::PairSelect| pair_victim(app.world(), hero, s);
    // The body looks south, away from the broken foe in the north.
    app.world_mut().get_mut::<Transform>(hero).unwrap().look_to(S, Vec3::Y);
    assert_eq!(victim(&app, &facing), None, "from the facing: behind");
    assert_eq!(victim(&app, &intent), None, "no lock, no stick: the facing");
    // The stick toward it: the direction meant.
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = N;
    assert_eq!(victim(&app, &intent), Some(foe));
    assert_eq!(victim(&app, &facing), None, "the default ignores the stick");
    // The stick away from it.
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = S;
    assert_eq!(victim(&app, &intent), None);
    // The lock first: locked on the broken foe, with the stick away from it, it is the foe that counts...
    app.world_mut().get_mut::<Intent>(hero).unwrap().target = Some(foe_id);
    assert_eq!(victim(&app, &intent), Some(foe), "the lock direction beats the stick");
    assert_eq!(victim(&app, &facing), None, "the default ignores the lock");
    // ...and locked on a foe in the other direction, the stick toward the broken one does not count.
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = N;
    app.world_mut().get_mut::<Intent>(hero).unwrap().target = Some(other_id);
    assert_eq!(victim(&app, &intent), None, "the lock points south");
}

#[test]
fn a_victim_killed_by_something_else_during_the_pair_is_lost_not_killed() {
    let (mut app, hero, foe) = arena();
    let ends = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = ends.clone();
    app.world_mut().add_observer(move |e: On<PairEnded>| into.lock().unwrap().push((e.event().end, e.event().killed)));
    // The finisher's own blow kills: `Lost`, and the event says it was the blow.
    app.world_mut().get_mut::<Health>(foe).unwrap().current = 10.0;
    finisher(&mut app, hero, foe);
    step(&mut app, 40);
    assert_eq!(ends.lock().unwrap().as_slice(), [(PairEnd::Lost, true)]);
    assert!(app.world().get::<shards_action::PairKilled>(hero).is_none(), "the mark goes with the pair");
    // A second pair whose victim dies of something else (a kill with no killer) is lost.
    let (mut app, hero, foe) = arena();
    let into = ends.clone();
    ends.lock().unwrap().clear();
    app.world_mut().add_observer(move |e: On<PairEnded>| into.lock().unwrap().push((e.event().end, e.event().killed)));
    finisher(&mut app, hero, foe);
    step(&mut app, 3);
    shards_action::kill(app.world_mut(), foe, None, None);
    step(&mut app, 2);
    assert_eq!(ends.lock().unwrap().as_slice(), [(PairEnd::Lost, false)]);
    assert!(!lines(&app, "(lost)").is_empty() && lines(&app, "(killed)").is_empty(), "{:#?}", journal(&app));
}
