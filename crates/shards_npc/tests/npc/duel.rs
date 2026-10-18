//! The NPC rules a duel needs: strike spacing between NPCs, conditional plan options, rest
//! semantics, combo continuation, decision cadence (perception's keep range is in
//! `perception.rs`). Counted in ticks, from the actions that start.

use bevy::prelude::*;
use shards_npc::{NpcStrike, NpcThink};
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

// --- N1: strike spacing --------------------------------------------------------------------

/// Two duelists on `plan` facing a hero between them, fighting by `director`.
fn pair(director: &str, plan: &str, seed: u64, setup: impl FnOnce(&mut App)) -> (App, [Entity; 2]) {
    pair_of("duelist", director, plan, seed, setup)
}

fn pair_of(moveset: &str, director: &str, plan: &str, seed: u64, setup: impl FnOnce(&mut App)) -> (App, [Entity; 2]) {
    let mut app = app_seeded(seed, |app| {
        record_starts(app);
        setup(app);
    });
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let make = |world: &mut World, z: f32| {
        npc(world, moveset, Npc::new("grunt", "grunt").with_plan(plan).with_director(director), Vec3::new(0.0, 0.0, z), if z > 0.0 { N } else { S })
    };
    let a = make(world, 1.2);
    let b = make(world, -1.2);
    (app, [a, b])
}

/// Every start of `action` by the two, sorted: `(tick, who)`.
fn all_starts(app: &App, who: [Entity; 2], action: &str) -> Vec<(u64, usize)> {
    let mut all: Vec<(u64, usize)> = (0..2).flat_map(|i| starts(app, who[i], action).into_iter().map(move |t| (t, i))).collect();
    all.sort();
    all
}

#[test]
fn strikes_of_different_npcs_start_at_least_the_spacing_apart() {
    for seed in [7, 11, 23] {
        let (mut app, who) = pair("spaced", "pair", seed, |_| {});
        step(&mut app, 1200);
        let jabs = all_starts(&app, who, "jab");
        assert!(jabs.len() >= 12 && jabs.iter().any(|(_, i)| *i == 0) && jabs.iter().any(|(_, i)| *i == 1), "seed {seed}: {jabs:?}");
        let gaps: Vec<u64> = jabs.windows(2).filter(|w| w[0].1 != w[1].1).map(|w| w[1].0 - w[0].0).collect();
        assert!(gaps.iter().all(|g| *g >= 42), "seed {seed}: 0.7 s = 42 ticks between different NPCs' strikes: {gaps:?} in {jabs:?}");
        assert!(gaps.contains(&42), "seed {seed}: and the second goes the tick the spacing runs out: {gaps:?}");
    }
}

#[test]
fn without_a_spacing_two_npcs_strike_in_the_same_tick() {
    let (mut app, who) = pair("loose", "pair", 7, |_| {});
    step(&mut app, 600);
    let jabs = all_starts(&app, who, "jab");
    assert!(jabs.windows(2).any(|w| w[0].0 == w[1].0 && w[0].1 != w[1].1), "{jabs:?}");
    // The neutral path leaves no trace: no component, no notes.
    assert!(who.iter().all(|e| app.world().get::<NpcStrike>(*e).is_none()));
}

#[test]
fn an_exempt_action_neither_waits_nor_counts() {
    let (mut app, who) = pair("spaced", "hopper", 7, |_| {});
    step(&mut app, 300);
    let hops = all_starts(&app, who, "hop_back");
    assert!(hops.windows(2).any(|w| w[0].0 == w[1].0 && w[0].1 != w[1].1), "retreats start together: {hops:?}");
    assert!(who.iter().all(|e| app.world().get::<NpcStrike>(*e).is_none()), "and are not noted");
    // A strike right after a retreat is not held back by it either.
}

#[test]
fn the_notes_say_who_struck_when_and_the_state_tool_shows_the_wait() {
    let (mut app, who) = pair("spaced", "pair", 7, |_| {});
    step(&mut app, 3);
    let mut seen_wait = false;
    for _ in 0..120 {
        step(&mut app, 1);
        let tick = app.world().resource::<ai_core::SimTick>().0;
        for i in 0..2 {
            let note = app.world().get::<NpcStrike>(who[i]);
            if let Some(n) = note {
                assert!(n.gated && n.last_start <= tick);
                assert_eq!(starts(&app, who[i], "jab").last().copied(), Some(n.last_start), "the note is the last start");
            }
        }
        for i in 0..2 {
            if let Some((by, left)) = shards_npc::wait_of(app.world(), who[i]) {
                seen_wait = true;
                assert_eq!(by, aid(&app, who[1 - i]));
                assert!((1..=42).contains(&left), "{left}");
            }
        }
    }
    assert!(seen_wait, "one of the duelists waited out the spacing at some point");
}

/// A hook that presses a strike itself asks `strike_ready` first.
fn press_when_ready(c: &NpcCtx, _: &Params, out: &mut NpcOutput) {
    if c.strike_ready() {
        out.press.push("light".into());
    }
}

#[test]
fn a_state_hook_that_asks_strike_ready_keeps_the_spacing_too() {
    // The first duelist strikes by token, the second presses by its own hook.
    let (mut app, who) = pair("spaced", "pair", 7, |app| {
        app.add_npc_state("press_when_ready", press_when_ready);
    });
    app.world_mut().entity_mut(who[1]).insert(Npc::new("presser", "grunt").with_director("spaced"));
    step(&mut app, 900);
    let jabs = all_starts(&app, who, "jab");
    assert!(jabs.iter().filter(|(_, i)| *i == 1).count() >= 5 && jabs.iter().filter(|(_, i)| *i == 0).count() >= 5, "{jabs:?}");
    let gaps: Vec<u64> = jabs.windows(2).filter(|w| w[0].1 != w[1].1).map(|w| w[1].0 - w[0].0).collect();
    // Two NPCs that are ready on the same tick both press it (every NPC decides on the same
    // world, a hook cannot see the other's press of this tick): a tie, not a short gap.
    assert!(gaps.iter().all(|g| *g == 0 || *g >= 42), "{gaps:?}");
    // Noted, but not held to the engine rule: the hook is in charge.
    assert!(app.world().get::<NpcStrike>(who[1]).is_some_and(|n| !n.gated));
}

// --- N2: conditional options and the rest clamp --------------------------------------------

fn rage(c: &NpcCtx, _: &Params) -> bool {
    c.health_fraction() < 0.5
}

fn cap_rest(c: &NpcCtx, _: &Params, out: &mut NpcOutput) {
    out.request_token = true;
    out.strike = true;
    out.waiting = true;
    out.leashed = true;
    out.face = c.target.map(|t| t.pos);
    if c.brain.since <= 1e-6 {
        out.rest_cap = Some(0.1);
    }
}

/// Which attacks `who` started, in order.
fn started(app: &App, who: Entity) -> Vec<(u64, String)> {
    app.world().resource::<Started>().0.iter().filter(|(_, e, a)| *e == who && a != "grunt_stagger").map(|(t, _, a)| (*t, a.clone())).collect()
}

#[test]
fn an_option_with_conditions_is_chosen_only_while_they_hold() {
    let (mut app, who) = duel("gated", |app| {
        app.add_npc_condition("rage", rage);
    });
    step(&mut app, 400);
    let calm: Vec<String> = started(&app, who).into_iter().map(|(_, a)| a).collect();
    assert!(calm.len() >= 3 && calm.iter().all(|a| a == "jab"), "only the calm option while healthy: {calm:?}");
    let opts = app.world().get::<shards_npc::NpcOptions>(who).expect("a plan with gated options has the component");
    assert_eq!(opts.allowed, vec!["calm"]);
    // Wounded, the other option opens and the first closes.
    app.world_mut().get_mut::<shards_action::Health>(who).unwrap().current = 20.0;
    let mark = started(&app, who).len();
    step(&mut app, 400);
    let fierce: Vec<String> = started(&app, who).into_iter().skip(mark).map(|(_, a)| a).collect();
    assert!(fierce.len() >= 3 && fierce.iter().skip(1).all(|a| a == "smash"), "only the fierce one when raging: {fierce:?}");
    assert_eq!(app.world().get::<shards_npc::NpcOptions>(who).unwrap().allowed, vec!["fierce"]);
}

#[test]
fn a_plan_without_conditions_carries_no_options_component() {
    let (mut app, who) = duel("pair", |_| {});
    step(&mut app, 60);
    assert!(app.world().get::<shards_npc::NpcOptions>(who).is_none());
}

#[test]
fn an_option_whose_condition_is_unknown_is_closed_and_named_once() {
    let (mut app, who) = duel("gated", |_| {});
    step(&mut app, 120);
    let named: Vec<String> = all_journal(&app).into_iter().filter(|l| l.contains("unknown condition `rage`")).collect();
    assert_eq!(named.len(), 1, "{named:?}");
    // `!rage` never holds either (an unknown condition never holds, negated or not): nothing opens.
    assert!(app.world().get::<shards_npc::NpcOptions>(who).unwrap().allowed.is_empty());
    assert!(started(&app, who).is_empty());
}

/// Plays `behavior` until its first attack ends, wounds the NPC, and returns the ticks from the
/// wound to the next attack's start.
fn wounded_then_strikes(behavior: &str) -> u64 {
    let mut app = app_with(|app| {
        record_starts(app);
        app.add_npc_state("cap_rest", cap_rest);
    });
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let who = npc(world, "duelist", Npc::new(behavior, "grunt").with_plan("long_rest").with_director("duel"), Vec3::new(0.0, 0.0, 1.2), N);
    // The first jab, then its token comes back and a rest of 1 s (60 ticks) begins.
    step(&mut app, 60);
    assert_eq!(started(&app, who).len(), 1);
    let rest = app.world().get::<shards_npc::NpcPlan>(who).unwrap().rest;
    assert!(rest > 0.5, "resting: {rest}");
    app.world_mut().get_mut::<shards_action::Health>(who).unwrap().current = 20.0;
    let wounded = app.world().resource::<ai_core::SimTick>().0;
    step(&mut app, 120);
    let next = started(&app, who)[1].0;
    next - wounded
}

#[test]
fn entering_a_state_can_clamp_the_rest_once() {
    // The wound is seen on the next tick; the new state clamps the 1 s rest to 0.1 s.
    let clamped = wounded_then_strikes("raging");
    let hooked = wounded_then_strikes("hooked");
    assert!((6..=9).contains(&clamped), "{clamped} ticks: 0.1 s of rest is 6, plus the tick it was seen on");
    assert_eq!(clamped, hooked, "a hook that sets rest_cap once does the same");
    // The control: the same wound without a state that clamps leaves the second to run.
    let control = wounded_then_strikes("grunt");
    assert!(control > clamped + 20, "{control} ticks without a clamp");
}

// --- N4: combo continuation ----------------------------------------------------------------

/// A lone combat fighter with `plan` against a hero 1.2 m away.
fn combo_duel(plan: &str, seed: u64) -> (App, Entity) {
    let mut app = app_seeded(seed, record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let who = npc(world, "combo", Npc::new("grunt", "grunt").with_plan(plan).with_director("duel"), Vec3::new(0.0, 0.0, 1.2), N);
    (app, who)
}

#[test]
fn a_chain_presses_at_the_start_of_the_recovery_and_the_combo_window_does_the_rest() {
    let (mut app, who) = combo_duel("chain_always", 7);
    step(&mut app, 300);
    let all: Vec<(u64, String)> = started(&app, who);
    let names: Vec<&str> = all.iter().map(|(_, a)| a.as_str()).collect();
    assert_eq!(&names[..4], ["swing_1", "swing_2", "swing_3", "swing_1"], "{all:?}");
    // swing_1 begins; its recovery starts 24 ticks in and the press is made on the first tick the NPC
    // sees it; the window opens 27 ticks in, within the press's 9 ticks of life: the continuation
    // starts at the first tick of the window.
    let (first, second, third) = (all[0].0, all[1].0, all[2].0);
    assert_eq!(second - first, 27, "{all:?}");
    assert_eq!(third - second, 27, "the continuation rolls the link written on its own option: {all:?}");
    // swing_3 has no link: it ends, rests, and the series starts over.
    assert!(all[3].0 - third > 48, "{all:?}");
    let story = journal(&app);
    assert!(story.iter().any(|l| l.contains("chains light after swing_1")), "{story:?}");
    assert!(story.iter().any(|l| l.contains("chains light after swing_2")));
}

#[test]
fn a_chain_whose_window_opens_after_the_press_died_never_fires() {
    let (mut app, who) = combo_duel("chain_slow", 7);
    step(&mut app, 400);
    let names: Vec<String> = started(&app, who).into_iter().map(|(_, a)| a).collect();
    assert!(names.len() >= 3 && names.iter().all(|a| a == "slow_1"), "{names:?}");
    // The press was made and died in the queue.
    assert!(lines(&app, "chains light after slow_1").len() >= 3);
    assert!(!lines(&app, "slow_2").iter().any(|l| l.contains("slow_2 ")), "no continuation");
}

#[test]
fn odds_of_zero_and_one_and_in_between_are_kept() {
    let (mut app, who) = combo_duel("chain_never", 7);
    step(&mut app, 600);
    assert!(started(&app, who).iter().all(|(_, a)| a == "swing_1"));
    assert!(app.world().get::<shards_npc::NpcChain>(who).is_none());
    let (mut app, who) = combo_duel("chain_none", 7);
    step(&mut app, 600);
    assert!(started(&app, who).iter().all(|(_, a)| a == "swing_1"));
    assert!(lines(&app, "chains").is_empty(), "nothing is journaled when no chain is written");
    // Half: about half of the attacks continue, over many seeds.
    let (mut first, mut second) = (0, 0);
    for seed in 0..40 {
        let (mut app, who) = combo_duel("chain_half", seed);
        step(&mut app, 900);
        for (_, a) in started(&app, who) {
            match a.as_str() {
                "swing_1" => first += 1,
                "swing_2" => second += 1,
                other => panic!("{other}"),
            }
        }
    }
    let share = second as f32 / first as f32;
    assert!((0.4..0.6).contains(&share), "{second} continued of {first} ({share:.2})");
}

#[test]
fn a_chain_is_cut_with_its_attack() {
    let (mut app, who) = combo_duel("chain_always", 7);
    // Into the first swing: the roll has been made.
    step(&mut app, 8);
    let chain = app.world().get::<shards_npc::NpcChain>(who).cloned().expect("rolled when the attack began");
    assert_eq!((chain.input.as_str(), chain.pressed), ("light", false));
    // A stagger cuts the attack: the chain goes with it and nothing is pressed.
    assert!(stagger(app.world_mut(), who, StaggerCause::Dev));
    step(&mut app, 60);
    assert!(app.world().get::<shards_npc::NpcChain>(who).is_none() || started(&app, who).len() > 1);
    assert!(lines(&app, "chains").is_empty(), "{:?}", journal(&app));
    assert!(started(&app, who).iter().take(1).all(|(_, a)| a == "swing_1"));
}

/// A hook that presses a strike without asking: its starts are noted but the planner cannot hold
/// them back.
fn unruly(_: &NpcCtx, _: &Params, out: &mut NpcOutput) {
    out.press.push("light".into());
}

#[test]
fn a_refused_continuation_ends_the_chain() {
    // The first duelist strikes whenever it can (a hook that never asks); the second plans its
    // strikes, and its continuations meet the other's starts.
    let (mut app, who) = pair_of("combo", "spaced", "chain_always", 7, |app| {
        app.add_npc_state("unruly", unruly);
    });
    app.world_mut().entity_mut(who[0]).insert(Npc::new("unruly_fighter", "grunt").with_director("spaced"));
    step(&mut app, 1500);
    let refused = lines(&app, "refused after");
    assert!(!refused.is_empty(), "no continuation was refused: {:?}", journal(&app).len());
    assert!(refused[0].contains("began a strike less than 42 ticks ago"), "{refused:?}");
    // The refusal ended the chain: the planner's fighter went on to rest instead of swinging the second blow.
    let planned = started(&app, who[1]);
    assert!(planned.iter().any(|(_, a)| a == "swing_1"), "{planned:?}");
    let firsts = planned.iter().filter(|(_, a)| a == "swing_1").count();
    let seconds = planned.iter().filter(|(_, a)| a == "swing_2").count();
    assert!(seconds < firsts, "refused chains did not continue: {firsts} first blows, {seconds} second");
    // The planner's own strikes stay 42 ticks from the other's that came before them (the engine rule
    // `npc.strike_spacing` held every tick, or `step` would have stopped).
    assert!(app.world().get::<shards_npc::NpcStrike>(who[0]).is_some_and(|n| !n.gated), "the hook's starts are not held to the rule");
}

#[test]
fn chain_numbers_are_checked() {
    use shards_content::{db::Source, ContentDb, ContentKinds};
    let mut kinds = ContentKinds::default();
    kinds.register::<shards_npc::AttackPlanDef>();
    kinds.register::<shards_action::ActionDef>();
    let text = r#"{ "p": (options: [(input: "light", chain: Some((input: "", chance: 1.5)))]) }"#;
    let db = ContentDb::build(&kinds, &[Source { path: "content/p.attack_plan.ron".into(), text: text.into() }]);
    let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].contains("options[0].chain names no input"), "{errors:?}");
    assert!(errors[1].contains("options[0].chain.chance is a probability in 0..1 (is 1.5)"), "{errors:?}");
}
