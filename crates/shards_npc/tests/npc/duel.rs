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
fn a_rest_counts_only_while_the_npc_is_free() {
    // The jab is cut after 10 ticks by a 36-tick stagger; the rest is 30 ticks.
    let (at, next) = cut_short("rest_plain", 10, 200);
    // The stagger is no rest, the 30 ticks start when it ends.
    assert_eq!(next.first().copied(), Some(at + 36 + 30), "stagger {at}: {next:?}");
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
        for e in who {
            if let Some(n) = app.world().get::<NpcStrike>(e) {
                assert!(n.gated && n.last_start <= tick);
                assert_eq!(starts(&app, e, "jab").last().copied(), Some(n.last_start), "the note is the last start");
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
    assert_eq!((chain.input.as_str(), chain.pressed), ("light", None));
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

// --- replays and snapshots -----------------------------------------------------------------

/// A hero and two combat fighters that use every rule of the duel at once.
fn everything(app: &mut App) -> [Entity; 2] {
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let make = |world: &mut World, z: f32| {
        npc(world, "combo", Npc::new("everything", "keeper").with_plan("duel_all").with_director("spaced"), Vec3::new(0.0, 0.0, z), if z > 0.0 { N } else { S })
    };
    [make(world, 1.2), make(world, -1.2)]
}

/// Plays the duel up to tick `until`, with a stagger of the first fighter at tick 70, in the middle
/// of an attack.
fn advance(app: &mut App, until: u64) {
    loop {
        let now = app.world().resource::<ai_core::SimTick>().0;
        if now >= until {
            break;
        }
        if now == 69 {
            call(app, "stagger", serde_json::json!({"id": "e2"}));
        }
        step(app, 1);
    }
}

#[test]
fn the_rules_of_the_duel_are_in_the_snapshot_and_a_loaded_fight_goes_on_the_same() {
    use ai_core::snapshot::{self, differences};
    let mut original = app_seeded(7, record_starts);
    let _who = everything(&mut original);
    let mut snapshots = Vec::new();
    for at in [30u64, 70, 71, 100, 140, 200, 333] {
        advance(&mut original, at);
        let snap = snapshot::capture(original.world()).unwrap();
        assert!(snap.left_out.is_empty(), "{:?}", snap.left_out);
        snapshots.push((at, snap));
    }
    advance(&mut original, 700);
    let last = snapshot::capture(original.world()).unwrap();
    // Every new piece of state reached a snapshot.
    for component in ["NpcStrike", "NpcThink", "NpcOptions", "NpcChain"] {
        assert!(
            snapshots.iter().any(|(_, s)| s.entities.iter().any(|e| e.components.keys().any(|k| k.ends_with(&format!("::{component}"))))),
            "{component} is in no snapshot"
        );
    }
    let story = journal(&original);
    for needle in ["chains light after", "returns its attack token"] {
        assert!(story.iter().any(|l| l.contains(needle)), "no `{needle}` in {story:#?}");
    }
    for (at, snap) in snapshots {
        let mut copy = app_seeded(7, record_starts);
        let report = snapshot::restore(copy.world_mut(), &snap, false).unwrap_or_else(|e| panic!("tick {at}: {e}"));
        assert!(report.exact(), "tick {at}: {:?}", report.differences);
        advance(&mut copy, 700);
        let diff = differences(&last, &snapshot::capture(copy.world()).unwrap());
        assert!(diff.is_empty(), "loaded at tick {at}:\n{}", diff.join("\n"));
    }
}

#[test]
fn a_recorded_duel_plays_back_identically() {
    use ai_core::replay::{self, Replay};
    let mut original = app_seeded(7, record_starts);
    let who = everything(&mut original);
    advance(&mut original, 700);
    let recording = replay::save(original.world());
    let recording: Replay = serde_json::from_str(&serde_json::to_string(&recording).unwrap()).unwrap();
    let mut copy = app_seeded(7, record_starts);
    everything(&mut copy);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    step(&mut copy, recording.end.tick);
    copy.update();
    let report = copy.world().resource::<ai_core::AiReplay>().report().cloned().expect("playback finished");
    assert!(report.identical(), "{report}");
    assert_eq!(journal(&original), journal(&copy));
    let left_out: Vec<String> = all_journal(&original).into_iter().filter(|l| l.contains("replay checks leave out")).collect();
    assert!(left_out.is_empty(), "{left_out:#?}");
    // The strikes were spaced all the way, and the interrupted fighter rested by its own range.
    assert!(started(&original, who[0]).len() >= 4 && started(&original, who[1]).len() >= 4);
}

// --- what the AI sees ----------------------------------------------------------------------

#[test]
fn npc_state_and_the_overlay_show_the_state_of_the_duel_rules() {
    let mut app = app_seeded(7, record_starts);
    let who = everything(&mut app);
    // The first duelist stays at its post, the hero steps back out of the sight range of
    // `keeper` (10 m) into its keep range (16 m) once it has been seen.
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..400 {
        step(&mut app, 1);
        for e in who {
            let who_id = id(&app, e);
            let out = call(&mut app, "npc_state", serde_json::json!({"id": who_id}));
            assert!(out.ok, "{}", text(&out));
            let v = out.result;
            for key in ["strike", "spacing", "think", "chain"] {
                if v.get(key).is_some_and(|x| !x.is_null()) {
                    seen.insert(key);
                }
            }
            assert!((v["think"]["every"].as_f64().unwrap() - 0.15).abs() < 1e-6, "{v}");
            assert!(v["plan"]["options"]["open"].is_array() && v["plan"]["options"]["closed"].is_array(), "{v}");
            assert_eq!(v["perception"]["keep_range"].as_f64(), Some(16.0));
        }
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), ["chain", "spacing", "strike", "think"]);
    // The overlay line says the same in a few words.
    let overlay = ok(&mut app, "npc_overlay", serde_json::json!({}));
    let overlay = text(&overlay);
    assert!(overlay.contains("think "), "{overlay}");
}

#[test]
fn an_npc_that_uses_none_of_the_rules_has_none_of_their_fields() {
    let mut app = app();
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let _ = hero;
    let g = grunt(app.world_mut(), "grunt", "grunt", "default", Vec3::new(0.0, 0.0, 3.0));
    step(&mut app, 120);
    let g_id = id(&app, g);
    let out = ok(&mut app, "npc_state", serde_json::json!({"id": g_id}));
    let v = out.result;
    for key in ["strike", "spacing", "think", "chain", "perception"] {
        assert!(v.get(key).is_none(), "{key} in {v}");
    }
    assert!(v["plan"].get("options").is_none());
    assert!(v["perceived"][0].get("kept").is_none());
}

#[test]
fn a_target_kept_in_sight_beyond_the_sight_range_is_marked() {
    let mut app = app();
    let world = app.world_mut();
    let watcher = npc(world, "grunt", Npc::new("watcher", "keeper"), Vec3::ZERO, N);
    let hero = player(world, 0, Vec3::new(0.0, 0.0, -9.0));
    step(&mut app, 1);
    teleport(&mut app, hero, Vec3::new(0.0, 0.0, -14.0));
    step(&mut app, 1);
    let w_id = id(&app, watcher);
    let v = ok(&mut app, "npc_state", serde_json::json!({"id": w_id})).result;
    assert_eq!(v["perception"], serde_json::json!({"sight_range": 10.0, "keep_range": 16.0}));
    assert_eq!(v["perceived"][0]["kept"], serde_json::json!(true), "{v}");
    let overlay = text(&ok(&mut app, "npc_overlay", serde_json::json!({})));
    assert!(overlay.contains("keeps e2 14.0m N"), "{overlay}");
}

/// A hook that presses a strike on tick 2, the tick a planner NPC on `pair` takes its first
/// token and would press.
fn press_at_two(c: &NpcCtx, _: &Params, out: &mut NpcOutput) {
    if c.tick == 2 {
        out.press.push("light".into());
    }
}

#[test]
fn the_planner_yields_to_a_hook_that_pressed_a_strike_on_the_same_tick() {
    for hooked in [0, 1] {
        let (mut app, who) = pair("spaced", "pair", 7, |app| {
            app.add_npc_state("press_at_two", press_at_two);
        });
        app.world_mut().entity_mut(who[hooked]).insert(Npc::new("presser_at_two", "grunt").with_director("spaced"));
        // `step` fails the test if the rule `npc.strike_spacing` is violated on the way.
        step(&mut app, 200);
        let hook = starts(&app, who[hooked], "jab");
        let planner = starts(&app, who[1 - hooked], "jab");
        assert_eq!(hook, vec![2], "hooked {hooked}: the hook pressed once, on tick 2");
        assert!(planner[0] >= 2 + 42, "hooked {hooked}: the planner waited out the spacing, it started on {}", planner[0]);
        assert_eq!(planner[0], 2 + 42, "and went the tick it ran out");
    }
}

#[test]
fn a_director_that_stops_spacing_leaves_no_notes() {
    let (mut app, who) = pair("spaced", "pair", 7, |_| {});
    step(&mut app, 120);
    assert!(who.iter().any(|e| app.world().get::<NpcStrike>(*e).is_some()));
    // The director is swapped for one without a spacing (a hot reload does the same).
    for e in who {
        app.world_mut().get_mut::<Npc>(e).unwrap().director = "loose".into();
    }
    step(&mut app, 3);
    assert!(who.iter().all(|e| app.world().get::<NpcStrike>(*e).is_none()), "the notes went with the spacing");
}

#[test]
fn two_chaining_planners_keep_the_spacing_through_their_continuations() {
    for seed in [7, 11, 23] {
        let (mut app, who) = pair_of("combo", "spaced", "chain_always", seed, |_| {});
        // `step` fails the test if the rule `npc.strike_spacing` is violated on the way.
        step(&mut app, 2400);
        let mut all: Vec<(u64, usize, String)> = Vec::new();
        for (i, e) in who.iter().enumerate() {
            all.extend(started(&app, *e).into_iter().map(|(t, a)| (t, i, a)));
        }
        all.sort();
        assert!(all.iter().any(|(_, _, a)| a == "swing_2") && all.iter().any(|(_, _, a)| a == "swing_3"), "seed {seed}: chains went through");
        let gaps: Vec<u64> = all.windows(2).filter(|w| w[0].1 != w[1].1).map(|w| w[1].0 - w[0].0).collect();
        assert!(!gaps.is_empty() && gaps.iter().all(|g| *g >= 42), "seed {seed}: {gaps:?}");
    }
}

#[test]
fn a_chain_rolls_the_odds_of_the_option_that_is_open() {
    let (mut app, who) = combo_duel("chain_odds", 7);
    app.add_npc_condition("rage", rage);
    step(&mut app, 400);
    let calm: Vec<String> = started(&app, who).into_iter().map(|(_, a)| a).collect();
    assert!(calm.len() >= 3 && calm.iter().all(|a| a == "swing_1"), "the calm option never chains: {calm:?}");
    app.world_mut().get_mut::<shards_action::Health>(who).unwrap().current = 20.0;
    let mark = started(&app, who).len();
    step(&mut app, 400);
    let fierce: Vec<String> = started(&app, who).into_iter().skip(mark).map(|(_, a)| a).collect();
    assert!(fierce.iter().any(|a| a == "swing_2"), "the fierce option chains every time: {fierce:?}");
}

#[test]
fn a_press_that_died_unused_is_no_start_for_the_others() {
    // `slow_1` presses its continuation 24 ticks in; its combo window opens at 42, after the
    // press (9 ticks of life) died: the other NPC is held back by the start, not by the press.
    let (mut app, who) = pair_of("combo", "spaced", "chain_slow", 7, |_| {});
    let first = loop {
        step(&mut app, 1);
        if let Some((t, i)) = all_starts(&app, who, "slow_1").first() {
            break (*t, *i);
        }
        assert!(app.world().resource::<ai_core::SimTick>().0 < 400, "no strike started");
    };
    let other = who[1 - first.1];
    step(&mut app, 36);
    let now = app.world().resource::<ai_core::SimTick>().0;
    assert_eq!(now, first.0 + 36);
    let chain = app.world().get::<shards_npc::NpcChain>(who[first.1]).expect("the chain is still there");
    assert!(chain.pressed.is_some_and(|p| p < now - 9), "the press is over: {chain:?}");
    assert_eq!(shards_npc::wait_of(app.world(), other).map(|(_, left)| left), Some(42 - 36), "counted from the start");
}

// --- E17: the press belongs to the decision tick -------------------------------------------

/// A duelist on `plan` with a decision cadence, 1.2 m from the hero.
fn deciding_duelist(plan: &str) -> (App, Entity) {
    let mut app = app_with(record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let who = npc(world, "grunt", Npc::new("grunt_think", "grunt").with_plan(plan).with_director("duel"), Vec3::new(0.0, 0.0, 1.2), N);
    (app, who)
}

/// The decision ticks (as `NpcThink.decided` shows them), the ticks `jab` started and the ticks
/// tokens were granted, over `ticks`.
fn decisions_jabs_grants(plan: &str, ticks: u64) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    let (mut app, who) = deciding_duelist(plan);
    let (mut decided, mut grants) = (Vec::<u64>::new(), Vec::<u64>::new());
    for _ in 0..ticks {
        step(&mut app, 1);
        if let Some(d) = app.world().get::<NpcThink>(who).and_then(|t| t.decided)
            && decided.last() != Some(&d)
        {
            decided.push(d);
        }
        if let Some(g) = app.world().get::<shards_npc::AttackToken>(who).and_then(|t| t.held.as_ref())
            && grants.last() != Some(&g.since)
        {
            grants.push(g.since);
        }
    }
    (decided, starts(&app, who, "jab"), grants)
}

#[test]
fn the_token_the_choice_and_the_press_share_a_decision_tick_when_the_plan_says_so() {
    let (decided, jabs, grants) = decisions_jabs_grants("decide_press", 500);
    assert!(jabs.len() >= 4 && decided.len() >= 20, "{jabs:?} {decided:?}");
    assert!(grants.len() >= 4 && grants.iter().all(|t| decided.contains(t)), "a token granted off the cadence: {grants:?} vs {decided:?}");
    assert!(jabs.iter().all(|t| decided.contains(t)), "a press on a tick that did not decide: {jabs:?} vs {decided:?}");
}

#[test]
fn without_the_rule_the_planner_presses_on_any_tick() {
    let (decided, jabs, grants) = decisions_jabs_grants("any_press", 500);
    assert!(jabs.len() >= 4, "{jabs:?}");
    assert!(jabs.iter().any(|t| !decided.contains(t)), "right after the token, off the cadence: {jabs:?} vs {decided:?}");
    assert!(grants.iter().any(|t| !decided.contains(t)), "{grants:?} vs {decided:?}");
}

#[test]
fn a_decision_cadence_of_nine_ticks_gives_a_press_every_nine_ticks_at_best() {
    let (decided, _, _) = decisions_jabs_grants("decide_press", 120);
    assert!(decided.windows(2).all(|w| w[1] - w[0] >= 9), "{decided:?}");
}
