//! The NPC rules a duel needs: strike spacing between NPCs, conditional plan options, rest
//! semantics, combo continuation, decision cadence (perception's keep range is in
//! `perception.rs`). Counted in ticks, from the actions that start.

use bevy::prelude::*;
use shards_npc::NpcThink;
use shards_npc::prelude::*;
use shards_action::{stagger, StaggerCause};

use crate::common::*;

/// Ticks at which `who` started `action`, in order.
fn starts(app: &App, who: Entity, action: &str) -> Vec<u64> {
    app.world().resource::<Started>().0.iter().filter(|(_, e, a)| *e == who && a == action).map(|(t, ..)| *t).collect()
}

/// A lone grunt with `plan` against a hero 1.2 m away that never moves.
fn duel(plan: &str, setup: impl FnOnce(&mut App)) -> (App, Entity) {
    let mut app = app_with(|app| {
        record_starts(app);
        setup(app);
    });
    let world = app.world_mut();
    let hero = player(world, 0, Vec3::ZERO);
    let _ = hero;
    let grunt = grunt(world, "grunt", plan, "duel", Vec3::new(0.0, 0.0, 1.2));
    (app, grunt)
}

// --- N3: rest semantics --------------------------------------------------------------------

/// Plays `plan` until the grunt's first jab, staggers it `after` ticks into it, and returns
/// (tick the stagger was forced, ticks of every jab started afterwards until `until`).
fn cut_short(plan: &str, after: u64, until: u64) -> (u64, Vec<u64>) {
    let (mut app, grunt) = duel(plan, |_| {});
    let first = loop {
        step(&mut app, 1);
        if let Some(t) = starts(&app, grunt, "jab").first() {
            break *t;
        }
        assert!(app.world().resource::<ai_core::SimTick>().0 < 400, "the first jab never started");
    };
    step(&mut app, after);
    assert!(stagger(app.world_mut(), grunt, StaggerCause::Dev));
    let at = app.world().resource::<ai_core::SimTick>().0;
    assert_eq!(at, first + after);
    step(&mut app, until);
    (at, starts(&app, grunt, "jab").into_iter().filter(|t| *t > first).collect())
}

#[test]
fn a_rest_counts_only_while_free_unless_it_runs_while_busy() {
    // The jab is cut after 10 ticks by a 36-tick stagger; the rest is 30 ticks.
    let (at, next) = cut_short("rest_plain", 10, 200);
    // Plain: the stagger is no rest, the 30 ticks start when it ends.
    assert_eq!(next.first().copied(), Some(at + 36 + 30), "stagger {at}: {next:?}");
    let (at, next) = cut_short("rest_busy", 10, 200);
    // Counting while busy, the whole rest ran during the stagger: the next jab starts the
    // first free tick after it.
    assert_eq!(next.first().copied(), Some(at + 36 + 1), "stagger {at}: {next:?}");
}

#[test]
fn an_interrupted_attack_rests_by_its_own_range_and_a_finished_one_by_the_rest() {
    let (at, next) = cut_short("rest_interrupted", 10, 200);
    assert_eq!(next.first().copied(), Some(at + 36 + 18), "interrupted: 0.3 s after the stagger: {next:?}");
    // Left alone, the same plan rests 0.5 s after the jab (54 ticks long).
    let (mut app, grunt) = duel("rest_interrupted", |_| {});
    step(&mut app, 400);
    let jabs = starts(&app, grunt, "jab");
    assert!(jabs.len() >= 4, "{jabs:?}");
    for w in jabs.windows(2) {
        assert_eq!(w[1] - w[0], 54 + 30, "finished: 0.5 s after the jab: {jabs:?}");
    }
}

#[test]
fn the_first_rest_is_a_pause_at_spawn() {
    let (mut plain, a) = duel("rest_plain", |_| {});
    step(&mut plain, 200);
    let (mut paused, b) = duel("rest_first", |_| {});
    step(&mut paused, 200);
    let (without, with) = (starts(&plain, a, "jab")[0], starts(&paused, b, "jab")[0]);
    // The pause is drawn on the NPC's first tick and counted from it: 60 ticks, so the jab
    // starts on tick 61; without it the grunt strikes on its second tick (the first one is
    // the brain's first run).
    assert_eq!((without, with), (2, 61));
    // Only once: the rests after the attacks are the plan's own.
    let jabs = starts(&paused, b, "jab");
    assert_eq!(jabs[1] - jabs[0], 54 + 30, "{jabs:?}");
}

// --- N5: decision cadence ------------------------------------------------------------------

/// The test's state hook: counts decision ticks in `NpcBrain.step`.
fn probe(c: &NpcCtx, _: &Params, out: &mut NpcOutput) {
    if c.deciding() {
        out.step = Some(c.brain.step + 1);
    }
}

fn watcher(behavior: &str) -> (App, Entity) {
    let mut app = app_with(|app| {
        app.add_npc_state("probe", probe);
    });
    let e = npc(app.world_mut(), "grunt", Npc::new(behavior, "grunt"), Vec3::new(0.0, 0.0, 20.0), N);
    (app, e)
}

fn decisions(app: &App, e: Entity) -> u32 {
    app.world().get::<NpcBrain>(e).unwrap().step
}

/// Ticks (1-based) on which the NPC decided, over `ticks` ticks.
fn decision_ticks(app: &mut App, e: Entity, ticks: u64) -> Vec<u64> {
    let mut out = Vec::new();
    for t in 1..=ticks {
        let before = decisions(app, e);
        step(app, 1);
        if decisions(app, e) > before {
            out.push(t);
        }
    }
    out
}

#[test]
fn a_think_cadence_decides_once_per_period_of_free_time() {
    let (mut app, e) = watcher("thinker");
    let ticks = decision_ticks(&mut app, e, 100);
    // The first free tick decides (tick 1 is the brain's first run: it enters its state and
    // decides), then every 0.15 s: nine ticks of 1/60 s.
    assert_eq!(&ticks[..4], &[1, 10, 19, 28], "{ticks:?}");
    assert!(ticks.windows(2).all(|w| w[1] - w[0] == 9), "{ticks:?}");
    assert!(app.world().get::<NpcThink>(e).is_some(), "the timer is a component of the NPC");
    // Without a cadence it decides every tick, and carries no timer.
    let (mut app, e) = watcher("eager");
    assert_eq!(decision_ticks(&mut app, e, 30), (1..=30).collect::<Vec<_>>());
    assert!(app.world().get::<NpcThink>(e).is_none());
}

#[test]
fn the_decision_timer_stands_still_while_the_npc_is_busy() {
    let (mut app, e) = watcher("thinker");
    let before = decision_ticks(&mut app, e, 14); // decided on 1 and 10; 4 ticks into the next period
    assert_eq!(before, vec![1, 10]);
    let left = app.world().get::<NpcThink>(e).unwrap().left;
    assert!(stagger(app.world_mut(), e, StaggerCause::Dev));
    // The stagger lasts 36 ticks: no decisions, and the timer does not move.
    let during = decision_ticks(&mut app, e, 35);
    assert!(during.is_empty(), "{during:?}");
    assert_eq!(app.world().get::<NpcThink>(e).unwrap().left, left);
    // Free again after the stagger; the rest of the period runs out, then it is regular.
    let after = decision_ticks(&mut app, e, 40);
    // 4 ticks of the period had run before the stagger, 5 are left: the fifth free tick after
    // the stagger (tick 14 + 36) decides, on tick 55; then every 9.
    assert_eq!(after, [6, 15, 24, 33], "free from the next tick, decides on the fifth");
}

#[test]
fn a_transition_can_wait_for_a_decision_tick() {
    let (mut app, e) = watcher("gated");
    let mut changes = Vec::new();
    for t in 1..=80 {
        let before = state(&app, e);
        step(&mut app, 1);
        if state(&app, e) != before {
            changes.push((t, state(&app, e)));
        }
    }
    let names: Vec<(u64, &str)> = changes.iter().map(|(t, s)| (*t, s.as_str())).collect();
    assert_eq!(names, [(1, "a"), (16, "b"), (31, "a"), (46, "b"), (61, "a"), (76, "b")], "a flip per decision, 0.25 s = 15 ticks apart");
}
