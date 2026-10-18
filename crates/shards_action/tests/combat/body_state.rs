//! Body state (B2b): the barrier, the damage-taken multipliers and the status proc rules.
//! Content: `assets/content/body.*.ron`. The attacker (`b_attacker`) stands at the origin facing
//! north; the victim 2 m north of it. Times follow the timer rule of the module: a timer of `s`
//! seconds set between ticks acts on the next `ceil(60 s)` ticks.

use std::sync::Mutex;

use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::status::{add_status_buildup, StatusEntry};
use shards_action::{
    clear_barrier, clear_damage_taken, deal_damage, extend_damage_taken, grant_barrier, kill, revive, set_damage_taken, ActionAppExt, Barrier,
    BarrierGranted, DamageCtx, DamageTaken, DamageTakenExpired, ProcGates, StatusApplied, StatusExpired,
};

use crate::common::*;
use crate::reactions::seen;

/// The events these tests look at.
#[derive(Resource, Default)]
struct Log {
    granted: Vec<(u64, f32, f32)>,
    expired: Vec<(u64, String)>,
    applied: Vec<(u64, String, u32)>,
    status_expired: Vec<(u64, String)>,
}

fn log(app: &App) -> &Log {
    app.world().resource::<Log>()
}

fn record(app: &mut App) {
    crate::reactions::record(app);
    app.init_resource::<Log>();
    app.add_observer(|e: On<BarrierGranted>, mut log: ResMut<Log>, tick: Res<ai_core::SimTick>| {
        log.granted.push((tick.0, e.event().amount, e.event().seconds));
    });
    app.add_observer(|e: On<DamageTakenExpired>, mut log: ResMut<Log>, tick: Res<ai_core::SimTick>| {
        log.expired.push((tick.0, e.event().key.clone()));
    });
    app.add_observer(|e: On<StatusApplied>, mut log: ResMut<Log>, tick: Res<ai_core::SimTick>| {
        log.applied.push((tick.0, e.event().status.clone(), e.event().stacks));
    });
    app.add_observer(|e: On<StatusExpired>, mut log: ResMut<Log>, tick: Res<ai_core::SimTick>| {
        log.status_expired.push((tick.0, e.event().status.clone()));
    });
}

/// An attacker and a victim of `victim` moveset, recording events.
fn arena(victim: &str) -> (App, Entity, Entity) {
    arena_with(victim, |_| {})
}

fn arena_with(victim: &str, setup: impl FnOnce(&mut App)) -> (App, Entity, Entity) {
    let mut app = app_with(|app| {
        record(app);
        setup(app);
    });
    let world = app.world_mut();
    let attacker = fighter(world, "Hero", "b_attacker", "heroes", Vec3::ZERO, N);
    let target = fighter(world, "Dummy", victim, "monsters", Vec3::new(0.0, 0.0, -2.0), S);
    (app, attacker, target)
}

fn barrier(app: &App, e: Entity) -> Option<Barrier> {
    app.world().get::<Barrier>(e).copied()
}

fn status(app: &App, e: Entity, id: &str) -> Option<StatusEntry> {
    app.world().get::<Statuses>(e).and_then(|s| s.get(id).cloned())
}

fn buildup(app: &mut App, e: Entity, id: &str, amount: f32) {
    add_status_buildup(app.world_mut(), e, id, amount);
}

#[test]
fn barrier_absorbs_before_health() {
    let (mut app, attacker, victim) = arena("b_victim");
    let (a, v) = (id(&app, attacker), id(&app, victim));
    grant_barrier(app.world_mut(), victim, 30.0, 5.0);
    // A hit of 10 comes out of the barrier alone: health is whole, the journal says so.
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 100.0);
    assert_eq!(barrier(&app, victim).unwrap().amount, 20.0);
    assert_eq!(lines(&app, "->"), [format!("6 {a} -> {v} b_hit -10 (hp 100) [barrier -10]")]);
    assert_eq!(seen(&app).damaged[0].absorbed, 10.0);
    // A hit of 40 empties it and hurts for the rest; the barrier is gone with its last point.
    step(&mut app, 20);
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 80.0);
    assert!(barrier(&app, victim).is_none(), "an empty barrier is removed");
    let big = lines(&app, "b_hit_big");
    assert_eq!(big.len(), 1);
    assert!(big[0].ends_with(&format!("{a} -> {v} b_hit_big -40 (hp 80) [barrier -20]")), "{big:?}");
    assert_eq!(seen(&app).damaged[1].absorbed, 20.0);
    // No barrier, no suffix.
    step(&mut app, 20);
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 70.0);
    assert!(!lines(&app, "b_hit -10").last().unwrap().contains("barrier"), "{:#?}", journal(&app));
}

#[test]
fn a_barrier_takes_a_blocked_hit_too() {
    let (mut app, attacker, victim) = arena("b_victim");
    grant_barrier(app.world_mut(), victim, 8.0, 5.0);
    hold(&mut app, victim, "block", true);
    step(&mut app, 30);
    press(&mut app, attacker, "heavy"); // 40, half through the guard: 20
    step(&mut app, 8);
    let blocked = lines(&app, "blocked");
    assert_eq!(blocked.len(), 1, "{:#?}", journal(&app));
    assert!(blocked[0].contains(": -12 hp,"), "what reached health: {}", blocked[0]);
    assert!(blocked[0].ends_with("(hp 88) [barrier -8]"), "{}", blocked[0]);
    assert_eq!(hp(&app, victim), 88.0);
}

#[test]
fn barrier_takes_the_larger_grant() {
    let (mut app, _, v) = arena("b_victim");
    grant_barrier(app.world_mut(), v, 30.0, 2.0);
    step(&mut app, 30);
    let left = barrier(&app, v).unwrap().left;
    assert!((left - 1.5).abs() < 0.02, "{left}");
    // A smaller grant keeps the pool and restarts the time.
    grant_barrier(app.world_mut(), v, 20.0, 2.0);
    assert_eq!(barrier(&app, v).unwrap(), Barrier { amount: 30.0, left: 2.0 });
    // A larger one raises the pool.
    grant_barrier(app.world_mut(), v, 50.0, 1.0);
    assert_eq!(barrier(&app, v).unwrap(), Barrier { amount: 50.0, left: 1.0 });
    // Nothing for 0, a negative number or no time; the event says what the pool is.
    grant_barrier(app.world_mut(), v, 0.0, 1.0);
    grant_barrier(app.world_mut(), v, -5.0, 1.0);
    grant_barrier(app.world_mut(), v, 5.0, 0.0);
    grant_barrier(app.world_mut(), v, f32::NAN, 1.0);
    assert_eq!(log(&app).granted, [(0, 30.0, 2.0), (30, 30.0, 2.0), (30, 50.0, 1.0)]);
}

#[test]
fn barrier_runs_out() {
    let (mut app, attacker, victim) = arena("b_victim");
    grant_barrier(app.world_mut(), victim, 30.0, 0.5);
    // 0.5 s is 30 ticks: it acts on the checks of the first 30 ticks and is gone for the 31st.
    step(&mut app, 29);
    assert!(barrier(&app, victim).is_some());
    step(&mut app, 1);
    assert!(barrier(&app, victim).is_none(), "removed after its last tick");
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 90.0, "nothing left to take the hit");
    assert!(!lines(&app, "->")[0].contains("barrier"));
    // Clearing by hand.
    grant_barrier(app.world_mut(), victim, 30.0, 5.0);
    clear_barrier(app.world_mut(), victim);
    assert!(barrier(&app, victim).is_none());
}

#[test]
fn a_cast_ward_grants_a_share_of_max_health() {
    let mut app = app_with(record);
    let caster = hero(app.world_mut(), "b_caster", Vec3::ZERO, N);
    let c = id(&app, caster);
    app.world_mut().entity_mut(caster).insert(Health::new(200.0));
    press(&mut app, caster, "technique.3");
    step(&mut app, 10);
    assert_eq!(barrier(&app, caster).unwrap().amount, 50.0, "a quarter of 200");
    assert_eq!(lines(&app, "barrier"), [format!("6 {c} barrier 50 for 2.0s")]);
    step(&mut app, 30);
    press(&mut app, caster, "technique.1");
    step(&mut app, 10);
    let b = barrier(&app, caster).unwrap();
    assert_eq!(b.amount, 50.0, "the larger of 50 and 30");
    assert!(b.left > 1.9, "and its time starts again: {b:?}");
}

#[test]
fn deal_damage_goes_through_the_barrier_and_can_kill() {
    let (mut app, attacker, victim) = arena("b_victim");
    let v = id(&app, victim);
    grant_barrier(app.world_mut(), victim, 15.0, 5.0);
    assert_eq!(deal_damage(app.world_mut(), victim, 10.0, Some(attacker)), 100.0);
    assert_eq!(barrier(&app, victim).unwrap().amount, 5.0);
    assert_eq!(deal_damage(app.world_mut(), victim, 25.0, Some(attacker)), 80.0, "5 from the barrier, 20 from health");
    assert!(barrier(&app, victim).is_none());
    // No event of a hit, no multiplier.
    set_damage_taken(app.world_mut(), victim, "core", 2.0, None);
    assert_eq!(deal_damage(app.world_mut(), victim, 10.0, None), 70.0);
    assert!(lines(&app, "->").is_empty());
    // It kills, crediting who it was told to.
    let a = id(&app, attacker);
    assert_eq!(deal_damage(app.world_mut(), victim, 500.0, Some(attacker)), 0.0);
    assert!(app.world().get::<Dead>(victim).is_some());
    assert_eq!(lines(&app, "died"), [format!("0 {v} died (killed by {a})")]);
    // The dead take nothing more; nor does anyone in god mode, or for a non-positive amount.
    assert_eq!(deal_damage(app.world_mut(), victim, 5.0, None), 0.0);
    let other = fighter(app.world_mut(), "Dummy", "b_victim", "monsters", Vec3::new(5.0, 0.0, 0.0), S);
    app.world_mut().entity_mut(other).insert(Invulnerable);
    assert_eq!(deal_damage(app.world_mut(), other, 50.0, None), 100.0);
    app.world_mut().entity_mut(other).remove::<Invulnerable>();
    assert_eq!(deal_damage(app.world_mut(), other, 0.0, None), 100.0);
    assert_eq!(deal_damage(app.world_mut(), other, f32::NAN, None), 100.0);
    assert_eq!(deal_damage(app.world_mut(), other, f32::INFINITY, None), 100.0, "not a finite amount");
    assert_eq!(deal_damage(app.world_mut(), other, 100.0, None), 0.0);
    assert!(app.world().get::<Dead>(other).is_some() && lines(&app, "died").len() == 2);
}

#[test]
fn taken_multiplies_and_expires_with_an_event() {
    let (mut app, attacker, victim) = arena("b_victim");
    let (a, v) = (id(&app, attacker), id(&app, victim));
    set_damage_taken(app.world_mut(), victim, "core", 1.5, Some(0.5));
    set_damage_taken(app.world_mut(), victim, "armor", 0.5, None);
    assert_eq!(app.world().get::<DamageTaken>(victim).unwrap().product(), 0.75);
    set_damage_taken(app.world_mut(), victim, "armor", 1.0, None);
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(lines(&app, "->"), [format!("6 {a} -> {v} b_hit -15 (hp 85)")]);
    // The window ends on a tick: 30 ticks from the set, one event, the key gone, the other kept.
    step(&mut app, 21);
    assert!(log(&app).expired.is_empty());
    step(&mut app, 1);
    assert_eq!(log(&app).expired, [(30, "core".to_owned())]);
    let t = app.world().get::<DamageTaken>(victim).unwrap();
    assert_eq!(t.0.len(), 1, "{t:?}");
    assert_eq!(t.get("armor").unwrap().left, None);
    // Clearing the last one removes the component, and a hit is a hit again.
    clear_damage_taken(app.world_mut(), victim, "armor");
    assert!(app.world().get::<DamageTaken>(victim).is_none());
    press(&mut app, attacker, "light");
    step(&mut app, 12);
    assert_eq!(hp(&app, victim), 75.0);
    assert_eq!(log(&app).expired.len(), 1, "a cleared key sends no event");
    // Zero is a number: the fighter takes nothing.
    set_damage_taken(app.world_mut(), victim, "ward", 0.0, Some(1.0));
    step(&mut app, 20);
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 75.0);
}

#[test]
fn bad_multipliers_are_ignored() {
    let (mut app, _, victim) = arena("b_victim");
    for (mult, secs) in [
        (f32::NAN, None),
        (-1.0, None),
        (f32::INFINITY, None),
        (1.5, Some(0.0)),
        (1.5, Some(-1.0)),
        (1.5, Some(f32::NAN)),
    ] {
        set_damage_taken(app.world_mut(), victim, "core", mult, secs);
        assert!(!extend_damage_taken(app.world_mut(), victim, "core", mult, secs));
    }
    assert!(app.world().get::<DamageTaken>(victim).is_none());
}

/// E46: a shorter window never replaces a longer one, and a permanent multiplier never cancels a
/// timed window; the game keeps no timer of its own.
#[test]
fn a_window_extends_to_the_later_end_and_a_permanent_set_never_cuts_it() {
    let (mut app, _, victim) = arena("b_victim");
    let state = |app: &App| app.world().get::<DamageTaken>(victim).and_then(|t| t.get("core").map(|m| (m.mult, m.left)));
    // The base: 1.2 for good. A window of 4 s over it takes the key at 1.35.
    assert!(extend_damage_taken(app.world_mut(), victim, "core", 1.2, None));
    assert_eq!(state(&app), Some((1.2, None)));
    assert!(extend_damage_taken(app.world_mut(), victim, "core", 1.35, Some(4.0)));
    assert_eq!(state(&app), Some((1.35, Some(4.0))));
    // (a) A shorter window keeps the longer end (the mult is the newest).
    step(&mut app, 60);
    assert!(extend_damage_taken(app.world_mut(), victim, "core", 1.4, Some(1.6)));
    let (mult, end) = state(&app).unwrap();
    assert_eq!(mult, 1.4);
    assert!((end.unwrap() - 3.0).abs() < 0.02, "the old end stays: {end:?}");
    // A longer window moves the end out.
    assert!(extend_damage_taken(app.world_mut(), victim, "core", 1.35, Some(5.0)));
    assert_eq!(state(&app), Some((1.35, Some(5.0))));
    // (b) A permanent set while the window runs changes nothing.
    assert!(!extend_damage_taken(app.world_mut(), victim, "core", 1.2, None));
    assert_eq!(state(&app), Some((1.35, Some(5.0))));
    // The window ends on its tick (300 ticks later, 360 in all); the base under it is back, with
    // no timer or copy of the base on the game's side. The event still fires.
    step(&mut app, 300);
    assert_eq!(log(&app).expired, [(360, "core".to_owned())]);
    assert_eq!(state(&app), Some((1.2, None)));
    assert_eq!(app.world().get::<DamageTaken>(victim).unwrap().get("core").unwrap().then, None);
    clear_damage_taken(app.world_mut(), victim, "core");
    assert!(extend_damage_taken(app.world_mut(), victim, "core", 1.2, None));
    assert_eq!(state(&app), Some((1.2, None)));
    // The plain set replaces whatever is there, a longer window included.
    set_damage_taken(app.world_mut(), victim, "core", 1.35, Some(4.0));
    set_damage_taken(app.world_mut(), victim, "core", 1.1, Some(1.0));
    assert_eq!(state(&app), Some((1.1, Some(1.0))));
    set_damage_taken(app.world_mut(), victim, "core", 1.2, None);
    assert_eq!(state(&app), Some((1.2, None)));
    // The reading side: `left` tells absent, permanent and timed apart.
    let t = app.world().get::<DamageTaken>(victim).unwrap();
    assert_eq!((t.left("core"), t.left("other")), (Some(None), None));
}

static SEEN: Mutex<Vec<(f32, Option<f32>, f32)>> = Mutex::new(Vec::new());

fn watch(ctx: &mut DamageCtx, _: &World) {
    SEEN.lock().unwrap().push((ctx.taken, ctx.guard_mult, ctx.damage));
    // A hook that rebuilds the damage folds `taken` and `guard_mult` in itself.
    ctx.damage = ctx.base * ctx.taken * ctx.guard_mult.unwrap_or(1.0) + 1.0;
}

#[test]
fn hook_sees_taken_and_guard_mult() {
    let (mut app, attacker, victim) = arena_with("b_victim", |app| {
        app.add_damage_hook(watch);
    });
    set_damage_taken(app.world_mut(), victim, "core", 1.5, None);
    set_damage_taken(app.world_mut(), victim, "mark", 2.0, None);
    hold(&mut app, victim, "block", true);
    step(&mut app, 30);
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    // 10 base: the guard halves it, the multipliers (3.0 together) triple it.
    let seen = SEEN.lock().unwrap().clone();
    assert_eq!(seen, [(3.0, Some(0.5), 10.0 * 0.5 * 3.0)]);
    assert_eq!(hp(&app, victim), 100.0 - (10.0 * 0.5 * 3.0 + 1.0));
}

#[test]
fn without_the_component_nothing_is_multiplied() {
    let (mut app, attacker, victim) = arena("b_victim");
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 90.0);
    assert!(app.world().get::<DamageTaken>(victim).is_none(), "and nothing was added to the fighter");
}

#[test]
fn proc_gate_swallows_and_resets() {
    let (mut app, _, victim) = arena("b_victim");
    buildup(&mut app, victim, "b_chill", 100.0);
    assert_eq!(log(&app).applied, [(0, "b_chill".to_owned(), 1)]);
    assert_eq!(app.world().get::<ProcGates>(victim).unwrap().0, [("b_chill".to_owned(), 2.0)]);
    // A second proc inside the 2 s: swallowed, and the bar is empty (not full and waiting).
    step(&mut app, 30);
    buildup(&mut app, victim, "b_chill", 100.0);
    assert_eq!(log(&app).applied.len(), 1, "swallowed");
    let chill = status(&app, victim, "b_chill").unwrap();
    assert_eq!(chill.buildup, 0.0);
    assert!(chill.active > 3.0 && chill.active < 4.0, "it was not restarted: {chill:?}");
    // Not even by waiting: the bar is empty, so nothing procs when the gate opens.
    step(&mut app, 100);
    assert!(app.world().get::<ProcGates>(victim).is_none(), "the gate ran out and went");
    assert_eq!(log(&app).applied.len(), 1);
    // Buildup that stays below the threshold is no proc; it adds up while the status is active.
    buildup(&mut app, victim, "b_chill", 60.0);
    assert_eq!(status(&app, victim, "b_chill").unwrap().buildup, 60.0, "buildup keeps coming while it is active");
}

#[test]
fn the_gate_opens_after_its_interval_on_a_tick() {
    let (mut app, _, victim) = arena("b_victim");
    buildup(&mut app, victim, "b_chill", 100.0);
    // 2 s is 120 ticks: gated on the checks of the first 120, open for the 121st.
    step(&mut app, 119);
    assert!(app.world().get::<ProcGates>(victim).is_some());
    buildup(&mut app, victim, "b_chill", 100.0);
    assert_eq!(log(&app).applied.len(), 1, "still gated");
    step(&mut app, 1);
    assert!(app.world().get::<ProcGates>(victim).is_none());
    buildup(&mut app, victim, "b_chill", 100.0);
    assert_eq!(log(&app).applied.len(), 2, "open");
}

#[test]
fn no_cooling_while_active() {
    let (mut app, _, victim) = arena("b_victim");
    buildup(&mut app, victim, "b_steady", 100.0);
    buildup(&mut app, victim, "b_steady", 60.0);
    // Active with 60 on the bar (decay 50/s would take it in 2 s): it does not cool.
    step(&mut app, 100);
    let steady = status(&app, victim, "b_steady").unwrap();
    assert!(steady.active > 0.0);
    assert_eq!(steady.buildup, 60.0);
    // Once it ran out it cools as before.
    step(&mut app, 200);
    assert!(status(&app, victim, "b_steady").is_none(), "expired and cooled down: {:?}", app.world().get::<Statuses>(victim));
}

#[test]
fn refresh_restarts_duration() {
    let (mut app, _, victim) = arena("b_victim");
    buildup(&mut app, victim, "b_chill", 100.0);
    step(&mut app, 150); // past the gate; 4 - 2.5 s left
    let before = status(&app, victim, "b_chill").unwrap();
    assert!(before.active > 1.4 && before.active < 1.6, "{before:?}");
    buildup(&mut app, victim, "b_chill", 100.0);
    let after = status(&app, victim, "b_chill").unwrap();
    assert_eq!((after.active, after.stacks), (4.0, 1), "restarted, no stack");
    assert_eq!(log(&app).applied, [(0, "b_chill".to_owned(), 1), (150, "b_chill".to_owned(), 1)]);
    assert_eq!(
        app.world().get::<ProcGates>(victim).unwrap().0,
        [("b_chill".to_owned(), 2.0)],
        "a re-proc closes the gate again"
    );
}

#[test]
fn uncapped_status_ignores_the_cap() {
    let (mut app, _, victim) = arena("b_victim");
    // The tuning's cap is two: a and b are active, c waits at full, a jolt procs all the same.
    buildup(&mut app, victim, "b_a", 100.0);
    buildup(&mut app, victim, "b_b", 100.0);
    buildup(&mut app, victim, "b_c", 100.0);
    assert_eq!(status(&app, victim, "b_c").unwrap().active, 0.0);
    assert_eq!(status(&app, victim, "b_c").unwrap().buildup, 100.0, "refused: waits at full (the old rule)");
    buildup(&mut app, victim, "b_jolt", 100.0);
    assert_eq!(log(&app).applied.iter().map(|a| a.1.as_str()).collect::<Vec<_>>(), ["b_a", "b_b", "b_jolt"]);
    // It runs out on a tick, and a capped status still has no slot.
    step(&mut app, 3);
    assert_eq!(log(&app).status_expired.iter().map(|a| a.1.as_str()).collect::<Vec<_>>(), ["b_jolt"]);
    assert_eq!(status(&app, victim, "b_c").unwrap().active, 0.0, "the jolt took no slot and freed none");
    buildup(&mut app, victim, "b_jolt", 100.0);
    assert_eq!(log(&app).applied.len(), 4, "and it procs again");
}

#[test]
fn per_fighter_cap() {
    // A moveset's own `max_statuses` beats the tuning's two.
    let (mut app, attacker, victim) = arena("b_capped");
    let other = fighter(app.world_mut(), "Dummy", "b_victim", "monsters", Vec3::new(6.0, 0.0, 0.0), S);
    press(&mut app, attacker, "technique.1"); // a hit with b_a and b_b at 100 each
    step(&mut app, 8);
    let statuses = app.world().get::<Statuses>(victim).unwrap();
    assert_eq!(statuses.active(), 1, "{statuses:?}");
    assert!(statuses.get("b_a").unwrap().active > 0.0);
    assert_eq!(statuses.get("b_b").unwrap().buildup, 100.0, "the second waits");
    // The same two on a fighter with the tuning's cap both apply.
    buildup(&mut app, other, "b_a", 100.0);
    buildup(&mut app, other, "b_b", 100.0);
    assert_eq!(app.world().get::<Statuses>(other).unwrap().active(), 2);
    // The waiting one takes the slot when the first runs out (the old rule, per fighter).
    step(&mut app, 300);
    assert!(log(&app).applied.iter().any(|a| a.1 == "b_b"), "{:?}", log(&app).applied);
}

#[test]
fn dev_tools_set_barriers_and_multipliers() {
    let (mut app, _, victim) = arena("b_victim");
    let v = id(&app, victim);
    let text = ok(&mut app, "barrier", json!({"id": v, "amount": 25, "seconds": 3}));
    assert_eq!(text, format!("{v} has a barrier of 25 for 3.0s"));
    assert_eq!(barrier(&app, victim).unwrap(), Barrier { amount: 25.0, left: 3.0 });
    assert!(fails(&mut app, "barrier", json!({"id": v, "amount": 25})).contains("seconds is required"));
    assert!(fails(&mut app, "barrier", json!({"id": v, "amount": -1, "seconds": 3})).contains("must be a number >= 0"));
    ok(&mut app, "barrier", json!({"id": v, "amount": 0}));
    assert!(barrier(&app, victim).is_none());
    // Something without health: the tool refuses it, the function ignores it.
    let crate_ = app.world_mut().spawn(ai_core::AiKind::new("Crate")).id();
    let c = id(&app, crate_);
    assert!(fails(&mut app, "barrier", json!({"id": c, "amount": 5, "seconds": 3})).contains("has no Health"));
    grant_barrier(app.world_mut(), crate_, 5.0, 3.0);
    assert!(barrier(&app, crate_).is_none());

    let text = ok(&mut app, "damage_taken", json!({"id": v, "key": "core", "mult": 1.35, "seconds": 3}));
    assert_eq!(text, format!("{v} now: takes x1.35 (core 3.0s)"));
    let text = ok(&mut app, "damage_taken", json!({"id": v, "key": "core", "mult": 1.2, "keep_later": true}));
    assert!(text.contains("unchanged"), "{text}");
    assert!(fails(&mut app, "damage_taken", json!({"id": v, "key": "core"})).contains("mult is required"));
    assert!(fails(&mut app, "damage_taken", json!({"id": v, "key": "core", "mult": 1.5, "seconds": 0})).contains("above 0"));
    ok(&mut app, "damage_taken", json!({"id": v, "key": "core", "clear": true}));
    assert!(app.world().get::<DamageTaken>(victim).is_none());
}

#[test]
fn old_statuses_are_untouched() {
    // A status that asks for none of the new rules behaves as it did, on the same call.
    let (mut app, _, victim) = arena("b_victim");
    buildup(&mut app, victim, "b_a", 60.0);
    step(&mut app, 30);
    assert_eq!(status(&app, victim, "b_a").unwrap().buildup, 60.0, "no decay_per_s: it stays");
    buildup(&mut app, victim, "b_a", 40.0);
    buildup(&mut app, victim, "b_a", 500.0);
    assert_eq!(status(&app, victim, "b_a").unwrap().active, 5.0, "ignored while active without stacks");
    assert!(app.world().get::<ProcGates>(victim).is_none(), "no min_proc_interval, no gate");
}

/// E46, the base: a permanent value set while a window runs is kept under it, so a base that
/// changes mid-window (a phase change) is what the key returns to; the game stores nothing.
#[test]
fn a_base_set_during_a_window_is_what_the_key_returns_to() {
    let (mut app, _, victim) = arena("b_victim");
    let t = |app: &App| app.world().get::<DamageTaken>(victim).and_then(|t| t.get("core").cloned()).unwrap();
    extend_damage_taken(app.world_mut(), victim, "core", 1.2, None);
    extend_damage_taken(app.world_mut(), victim, "core", 1.35, Some(1.0));
    assert_eq!(t(&app).then, Some(1.2));
    // Phase 2: the base becomes 1.5 while the window runs. The window keeps the key.
    assert!(!extend_damage_taken(app.world_mut(), victim, "core", 1.5, None), "the multiplier in force did not change");
    let m = t(&app);
    assert_eq!((m.mult, m.left, m.then), (1.35, Some(1.0), Some(1.5)));
    assert_eq!(shards_action::body::describe_damage_taken(app.world().get::<DamageTaken>(victim).unwrap()), "takes x1.35 (core 1.0s, then x1.50)");
    // A second window changes the mult and the end, not the base.
    step(&mut app, 30);
    extend_damage_taken(app.world_mut(), victim, "core", 1.4, Some(1.0));
    let m = t(&app);
    assert_eq!((m.mult, m.then), (1.4, Some(1.5)));
    step(&mut app, 59);
    assert_eq!(t(&app).mult, 1.4);
    step(&mut app, 1);
    let m = t(&app);
    assert_eq!((m.mult, m.left, m.then), (1.5, None, None), "back to the new base, permanent");
    assert_eq!(log(&app).expired, [(90, "core".to_owned())]);
    // The plain set drops the base: a window the game set on purpose to replace everything.
    set_damage_taken(app.world_mut(), victim, "core", 2.0, Some(1.0));
    assert_eq!(t(&app).then, None);
    step(&mut app, 60);
    assert!(app.world().get::<DamageTaken>(victim).is_none());
}

#[test]
fn a_regrant_restarts_the_time_even_when_shorter() {
    // The design's rule (B2b 4.10): the pool is the larger, the time is the newest grant's.
    let (mut app, _, v) = arena("b_victim");
    grant_barrier(app.world_mut(), v, 30.0, 5.0);
    step(&mut app, 60);
    grant_barrier(app.world_mut(), v, 10.0, 1.0);
    assert_eq!(barrier(&app, v).unwrap(), Barrier { amount: 30.0, left: 1.0 });
}

#[test]
fn a_broken_guard_and_a_spill_report_what_the_barrier_took() {
    let (mut app, attacker, victim) = arena("b_victim");
    // Spill with the guard share and a multiplier together: 40 x 0.5 (guard) x 1.5 (taken) = 30;
    // the barrier takes 12, health 18.
    grant_barrier(app.world_mut(), victim, 12.0, 5.0);
    set_damage_taken(app.world_mut(), victim, "core", 1.5, None);
    hold(&mut app, victim, "block", true);
    step(&mut app, 30);
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    let blocked = lines(&app, "blocked");
    assert_eq!(blocked.len(), 1, "{:#?}", journal(&app));
    assert!(blocked[0].ends_with("(hp 82) [barrier -12]"), "{}", blocked[0]);
    assert_eq!(hp(&app, victim), 82.0);
    assert!(barrier(&app, victim).is_none());
}

#[test]
fn guard_broken_carries_the_absorbed_share() {
    let (mut app, attacker, victim) = arena("b_victim");
    // No stamina left: the guard breaks on the hit, and the barrier takes its share first.
    grant_barrier(app.world_mut(), victim, 5.0, 5.0);
    hold(&mut app, victim, "block", true);
    step(&mut app, 30);
    app.world_mut().get_mut::<Stamina>(victim).unwrap().current = 0.0;
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    let broken = lines(&app, "broke");
    let all = journal(&app);
    assert!(!broken.is_empty(), "{all:#?}");
    assert!(broken[0].contains("[barrier -5]"), "{}", broken[0]);
}

#[test]
fn damage_taken_expiries_come_out_in_ai_id_then_key_order() {
    let (mut app, attacker, victim) = arena("b_victim");
    // Set in the opposite order of what is expected, on the later fighter first.
    for key in ["zeta", "alpha", "mid"] {
        set_damage_taken(app.world_mut(), victim, key, 1.1, Some(0.5));
    }
    for key in ["omega", "beta"] {
        set_damage_taken(app.world_mut(), attacker, key, 1.1, Some(0.5));
    }
    
    app.init_resource::<Order>();
    app.add_observer(|e: On<DamageTakenExpired>, ids: Query<&ai_core::AiId>, mut order: ResMut<Order>| {
        order.0.push((ids.get(e.event().entity).unwrap().0, e.event().key.clone()));
    });
    step(&mut app, 30);
    let got = app.world().resource::<Order>().0.clone();
    let (ai, vi) = (app.world().get::<ai_core::AiId>(attacker).unwrap().0, app.world().get::<ai_core::AiId>(victim).unwrap().0);
    let key = |s: &str| s.to_owned();
    assert_eq!(got, [(ai, key("beta")), (ai, key("omega")), (vi, key("alpha")), (vi, key("mid")), (vi, key("zeta"))]);
}

#[derive(Resource, Default)]
struct Order(Vec<(u32, String)>);

#[test]
fn deal_damage_does_nothing_on_a_client() {
    let (mut app, _, victim) = arena("b_victim");
    app.world_mut().insert_resource(shards_core::NetRole::Client);
    assert_eq!(deal_damage(app.world_mut(), victim, 500.0, None), 100.0);
    assert!(app.world().get::<Dead>(victim).is_none());
    app.world_mut().remove_resource::<shards_core::NetRole>();
    assert_eq!(deal_damage(app.world_mut(), victim, 10.0, None), 90.0);
}

#[test]
fn the_dead_barrier_multipliers_and_gates_are_frozen_until_a_revive() {
    let (mut app, attacker, victim) = arena("b_victim");
    grant_barrier(app.world_mut(), victim, 30.0, 1.0);
    set_damage_taken(app.world_mut(), victim, "core", 2.0, Some(1.0));
    buildup(&mut app, victim, "b_chill", 100.0);
    let gate_before = app.world().get::<ProcGates>(victim).unwrap().0.clone();
    kill(app.world_mut(), victim, Some(attacker), None);
    let (barrier_at_death, taken_at_death) = (barrier(&app, victim).unwrap(), app.world().get::<DamageTaken>(victim).unwrap().clone());
    // Far past every timer: nothing ran down on the corpse, nothing expired.
    step(&mut app, 300);
    assert_eq!(barrier(&app, victim), Some(barrier_at_death));
    assert_eq!(*app.world().get::<DamageTaken>(victim).unwrap(), taken_at_death);
    assert!(app.world().get::<ProcGates>(victim).is_some());
    assert!(log(&app).expired.is_empty());
    // A revive resumes them where they stood.
    app.world_mut().get_mut::<Health>(victim).unwrap().current = 100.0;
    assert!(revive(app.world_mut(), victim));
    let resumed = barrier(&app, victim).unwrap();
    assert_eq!(resumed.left, 1.0, "frozen at its full time through 300 ticks");
    assert_eq!(app.world().get::<ProcGates>(victim).unwrap().0, gate_before, "the gate did not run down either");
    step(&mut app, 70);
    assert!(barrier(&app, victim).is_none() && app.world().get::<DamageTaken>(victim).is_none());
    assert_eq!(log(&app).expired.len(), 1);
}
