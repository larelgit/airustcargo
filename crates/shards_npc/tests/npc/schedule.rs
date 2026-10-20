//! The boss scheduler's gates (N8a2): the attack planner's options carry an `angle`, a `players`
//! window, `max_repeats` and `forbid_after`, one judge (`refusal`) asks them for the director's
//! grant and the planner alike, `npc_schedule` says why an option is refused, and the engine rule
//! `npc.plan_sequence` holds the planner to its own history. Test content in
//! `assets/content/schedule.*.ron`; every test runs with the engine's rules on, so a step that
//! breaks one fails.

use ai_core::replay::{self, parts_at};
use ai_core::{AgentSessions, Scope};
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_action::{Cooldown, Cooldowns, Dead};
use shards_motion::CharacterParams;
use shards_npc::prelude::*;
use shards_npc::{set_plan_cooldown, CooldownSet, Grant, NpcPlan, HOST_OWNS_PLANS};

use crate::common::*;

/// A hero at the origin and one NPC `at` facing `facing`, fighting by director `duel` (one token).
fn arena(app: &mut App, plan: &str, behavior: &str, at: Vec3, facing: Vec3) -> (Entity, Entity) {
    let world = app.world_mut();
    let hero = player(world, 0, Vec3::ZERO);
    let npc = npc(world, "sched", Npc::new(behavior, "grunt").with_plan(plan).with_director("duel"), at, facing);
    (hero, npc)
}

/// The same with the NPC 1.2 m south of the hero, facing it.
fn duel(app: &mut App, plan: &str, behavior: &str) -> Entity {
    arena(app, plan, behavior, Vec3::new(0.0, 0.0, 1.2), N).1
}

fn plan_of(app: &App, e: Entity) -> NpcPlan {
    app.world().get::<NpcPlan>(e).unwrap().clone()
}

fn grant_attack(app: &App, e: Entity) -> Option<String> {
    app.world().get::<AttackToken>(e).unwrap().held.as_ref().and_then(|g| g.attack.clone())
}

/// The actions `who` started, in order.
fn actions(app: &App, who: Entity) -> Vec<String> {
    app.world().resource::<Started>().0.iter().filter(|(_, e, _)| *e == who).map(|(_, _, a)| a.clone()).collect()
}

/// `npc_schedule` as the dev, parsed.
fn schedule(app: &mut App, args: Value) -> Value {
    let out = ok(app, "npc_schedule", args);
    out.result
}

fn option<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["options"].as_array().unwrap().iter().find(|o| o["name"] == name).unwrap_or_else(|| panic!("no option {name} in {report}"))
}

fn eligible(report: &Value) -> Vec<String> {
    report["options"].as_array().unwrap().iter().filter(|o| o["eligible"] == true).map(|o| o["name"].as_str().unwrap().to_owned()).collect()
}

// --- 12.2 run 1: repeats ---------------------------------------------------------------------

#[test]
fn an_option_is_never_chosen_more_often_in_a_row_than_its_max_repeats() {
    let mut app = app_seeded(11, record_starts);
    let grunt = duel(&mut app, "sched_repeats", "grunt");
    step(&mut app, 3000);
    let played = actions(&app, grunt);
    assert!(played.len() >= 30, "{} attacks in 3000 ticks", played.len());
    assert!(played.iter().any(|a| a == "jab") && played.iter().any(|a| a == "smash"), "both options are chosen: {played:?}");
    for w in played.windows(3) {
        assert!(!(w[0] == "jab" && w[1] == "jab" && w[2] == "jab"), "three jabs in a row: {played:?}");
    }
    // The limit is what stops them: the same plan without it plays three in a row (jab is 3 to 1).
    let mut free = app_seeded(11, record_starts);
    let other = duel(&mut free, "sched_free", "grunt");
    step(&mut free, 3000);
    let played = actions(&free, other);
    assert!(played.windows(3).any(|w| w.iter().all(|a| a == "jab")), "{played:?}");
    // The history keeps as many as the plan needs: the largest max_repeats.
    assert_eq!(plan_of(&app, grunt).history.len(), 2);
    assert_eq!(plan_of(&free, other).history.len(), 1);
}

// --- 12.2 run 2: angle -----------------------------------------------------------------------

/// An NPC `degrees` off the direction of the hero, 1.2 m south of it, that turns 30 degrees a second.
fn slow_turner(app: &mut App, plan: &str, degrees: f32) -> Entity {
    let facing = Quat::from_rotation_y(degrees.to_radians()) * N;
    let (_, e) = arena(app, plan, "grunt_think", Vec3::new(0.0, 0.0, 1.2), facing);
    app.world_mut().entity_mut(e).insert(CharacterParams { turn_speed_deg: 30.0, locked_turn_speed_deg: 30.0, ..default() });
    e
}

#[test]
fn a_target_behind_is_answered_by_the_option_that_needs_it() {
    let mut app = app_seeded(3, record_starts);
    let e = slow_turner(&mut app, "sched_angle", 150.0);
    // The first decision after its grant: the only option whose angle holds is `turn`.
    let mut chosen = None;
    for _ in 0..60 {
        step(&mut app, 1);
        if let Some(name) = grant_attack(&app, e) {
            chosen = Some(name);
            break;
        }
    }
    assert_eq!(chosen.as_deref(), Some("turn"));
    // It is a slow body: nine decisions' worth of turning has not brought it round.
    let facing = app.world().get::<Transform>(e).unwrap().forward();
    let angle = shards_npc::target_angle(*facing, feet(&app, e), Vec3::ZERO);
    assert!(angle > 95.0, "{angle} degrees off");
}

#[test]
fn a_target_in_front_is_never_answered_with_the_option_for_behind() {
    let mut app = app_seeded(3, record_starts);
    let e = slow_turner(&mut app, "sched_angle", 10.0);
    step(&mut app, 3000);
    let played = actions(&app, e);
    assert!(played.len() >= 20, "{played:?}");
    assert!(played.iter().all(|a| a == "jab"), "`turn` (the smash) needs 95-180 degrees: {played:?}");
}

#[test]
fn the_tool_judges_an_angle_without_moving_anything() {
    let mut app = app_seeded(3, record_starts);
    let e = slow_turner(&mut app, "sched_angle", 10.0);
    step(&mut app, 2);
    let id = id(&app, e);
    let before = feet(&app, e);
    let behind = schedule(&mut app, json!({"id": id, "angle": 150.0}));
    assert_eq!(eligible(&behind), ["turn"], "{behind}");
    assert_eq!(behind["what_if"], true);
    assert!(option(&behind, "light")["refused"].as_str().unwrap().starts_with("angle: the target is 150 degrees off the facing; this option needs 0-90"), "{behind}");
    let front = schedule(&mut app, json!({"id": id, "angle": 10.0}));
    assert_eq!(eligible(&front), ["light"], "{front}");
    assert!(option(&front, "turn")["refused"].as_str().unwrap().starts_with("angle:"), "{front}");
    assert_eq!(feet(&app, e), before);
    // Measured, it says what the body has: roughly where it was turned to.
    let measured = schedule(&mut app, json!({"id": id}));
    assert!(measured.get("what_if").is_none());
    assert!(measured["angle"].as_f64().unwrap() <= 10.0, "{measured}");
}

// --- 12.2 run 6: a phase swap ------------------------------------------------------------------

#[test]
fn a_plan_swap_keeps_the_history_and_the_cooldowns_it_shares_by_name() {
    let mut app = app_seeded(5, record_starts);
    let e = duel(&mut app, "sched_phase_1", "grunt");
    // Until its first swing has started: `light` is remembered, on its 5 s cooldown.
    for _ in 0..200 {
        step(&mut app, 1);
        if !plan_of(&app, e).history.is_empty() {
            break;
        }
    }
    let was = plan_of(&app, e);
    assert!(was.history.len() == 1 && was.cooldown("light") > 4.0, "{was:?}");
    let name = was.history[0].name.clone();
    // The next phase: another plan with `light` in it. The memory is the NPC's, not the plan's.
    app.world_mut().get_mut::<Npc>(e).unwrap().plan = Some("sched_phase_2".into());
    let id = id(&app, e);
    let report = schedule(&mut app, json!({"id": id}));
    assert_eq!(report["plan"], "sched_phase_2");
    assert_eq!(report["history"], json!([name]));
    let light = option(&report, "light");
    assert!(light["refused"].as_str().unwrap().starts_with("cooldown: "), "{light}");
    // With the cooldown gone, the repeat count still refuses it, and `spin` is what remains.
    app.world_mut().get_mut::<NpcPlan>(e).unwrap().cooldowns.clear();
    let report = schedule(&mut app, json!({"id": id}));
    assert!(option(&report, "light")["refused"].as_str().unwrap().starts_with("repeats: chosen 1 times in a row, max_repeats 1"), "{report}");
    assert_eq!(eligible(&report), ["spin"]);
    step(&mut app, 600);
    let played = actions(&app, e);
    assert!(played.windows(2).all(|w| w[0] != "jab" || w[1] != "jab"), "light never twice in a row across the swap: {played:?}");
}

#[test]
fn a_choice_the_new_plan_lacks_is_dropped() {
    let mut app = app_seeded(5, record_starts);
    let (_, e) = arena(&mut app, "sched_far", "grunt", Vec3::new(0.0, 0.0, 5.0), N);
    for _ in 0..60 {
        step(&mut app, 1);
        if grant_attack(&app, e).is_some() {
            break;
        }
    }
    assert_eq!(grant_attack(&app, e).as_deref(), Some("far"), "walking up to the window of its only option");
    app.world_mut().get_mut::<Npc>(e).unwrap().plan = Some("sched_near".into());
    step(&mut app, 1);
    assert_eq!(grant_attack(&app, e).as_deref(), Some("near"), "`far` is not in the new plan: the choice is made again");
}

// --- 12.2 run 7: the reset and cooldown API (N8c) ---------------------------------------------------

/// Until `e`'s first swing has started and is remembered.
fn first_swing(app: &mut App, e: Entity) {
    for _ in 0..300 {
        step(app, 1);
        if !plan_of(app, e).history.is_empty() {
            return;
        }
    }
    panic!("no swing in 300 ticks: {:?}", plan_of(app, e));
}

#[test]
fn a_reset_forgets_the_history_and_the_cooldowns_and_says_so() {
    let mut app = app_seeded(5, record_starts);
    let e = duel(&mut app, "sched_phase_1", "grunt");
    first_swing(&mut app, e);
    let was = plan_of(&app, e);
    assert!(was.history.len() == 1 && was.cooldown("light") > 4.0 && was.attacks == 1, "{was:?}");
    let who = id(&app, e);
    let out = ok(&mut app, "npc_plan_reset", json!({"id": who}));
    assert_eq!(text(&out), format!("{who} plan reset: 1 attack forgotten, 1 cooldown cleared"));
    let now = plan_of(&app, e);
    assert!(now.history.is_empty() && now.cooldowns.is_empty(), "{now:?}");
    assert_eq!((now.attacks, now.rest), (was.attacks, was.rest), "the count and the rest stay");
    let said = lines(&app, "plan reset");
    assert!(said.len() == 1 && said[0].ends_with(&format!(" {who} plan reset (npc_plan_reset): 1 attack forgotten, 1 cooldown cleared")), "{said:?}");
    // A second reset has nothing to forget: harmless, and no journal line.
    let out = ok(&mut app, "npc_plan_reset", json!({"id": who}));
    assert_eq!(text(&out), format!("{who} plan reset: nothing to forget"));
    assert_eq!(lines(&app, "plan reset").len(), 1);
}

#[test]
fn a_reset_lets_the_plan_choose_again_what_its_history_refused() {
    // `light` at most once in a row and nothing else: after one swing the plan has nothing to choose
    // until it forgets it. The rule `npc.plan_sequence` judges every press throughout.
    let mut app = app_seeded(9, record_starts);
    let e = duel(&mut app, "sched_strict", "grunt");
    step(&mut app, 300);
    assert_eq!(actions(&app, e), ["jab"], "one swing, then the repeat rule refuses the only option");
    let who = id(&app, e);
    ok(&mut app, "npc_plan_reset", json!({"id": who}));
    step(&mut app, 300);
    assert_eq!(actions(&app, e), ["jab", "jab"], "forgotten, chosen again, once");
}

#[test]
fn a_cooldown_is_set_at_least_or_exactly() {
    let mut app = app_seeded(5, record_starts);
    let (hero, e) = arena(&mut app, "sched_all", "grunt", Vec3::new(0.0, 0.0, 1.2), N);
    let (who, hero) = (id(&app, e), id(&app, hero));
    let set = |app: &mut App, args: Value| text(&ok(app, "npc_plan_cooldown", args));
    let left = |app: &App| plan_of(app, e).cooldowns.iter().map(|c| (c.name.clone(), c.left)).collect::<Vec<_>>();
    assert_eq!(set(&mut app, json!({"id": who, "option": "turn", "seconds": 3.0})), format!("{who} turn: cooldown 3.00 s (was 0.00 s)"));
    assert_eq!(set(&mut app, json!({"id": who, "option": "heavy", "seconds": 4.0})), format!("{who} heavy: cooldown 4.00 s (was 0.00 s)"));
    assert_eq!(left(&app), [("heavy".to_owned(), 4.0), ("turn".to_owned(), 3.0)], "sorted by name, as remember keeps them");
    // At least never shortens; it lengthens.
    let args = json!({"id": who, "option": "heavy", "seconds": 2.0, "mode": "at_least"});
    assert_eq!(set(&mut app, args), format!("{who} heavy: cooldown 4.00 s (was 4.00 s)"));
    let args = json!({"id": who, "option": "heavy", "seconds": 6.0, "mode": "at_least"});
    assert_eq!(set(&mut app, args), format!("{who} heavy: cooldown 6.00 s (was 4.00 s)"));
    // Exactly shortens, and 0 takes the entry out.
    set(&mut app, json!({"id": who, "option": "heavy", "seconds": 1.0, "mode": "exactly"}));
    assert_eq!(set(&mut app, json!({"id": who, "option": "heavy", "seconds": 0.0})), format!("{who} heavy: cooldown 0.00 s (was 1.00 s)"));
    // A cooldown the planner would let go on its next tick (1e-4 s or less) is none.
    let args = json!({"id": who, "option": "heavy", "seconds": 0.00005});
    assert_eq!(set(&mut app, args), format!("{who} heavy: cooldown 0.00 s (was 0.00 s)"));
    assert_eq!(left(&app), [("turn".to_owned(), 3.0)]);
    // A line per change, none for the call that changed nothing.
    let changed: Vec<String> = lines(&app, " cooldown ").into_iter().map(|l| l.split_once(' ').unwrap().1.to_owned()).collect();
    assert_eq!(
        changed,
        [
            format!("{who} turn cooldown 3.00 s (was 0.00 s)"),
            format!("{who} heavy cooldown 4.00 s (was 0.00 s)"),
            format!("{who} heavy cooldown 6.00 s (was 4.00 s)"),
            format!("{who} heavy cooldown 1.00 s (was 6.00 s)"),
            format!("{who} heavy cooldown 0.00 s (was 1.00 s)"),
        ]
    );
    // The planner reads it: `turn` is refused by its cooldown (a tick of it may have run since).
    let report = schedule(&mut app, json!({"id": who, "target": hero, "angle": 150}));
    let refused = option(&report, "turn")["refused"].as_str().unwrap().to_owned();
    assert!(refused.starts_with("cooldown: 2.9") || refused.starts_with("cooldown: 3.00"), "{report}");
}

#[test]
fn the_plan_calls_say_how_to_fix_a_bad_call() {
    let mut app = app_seeded(5, record_starts);
    let (hero, e) = arena(&mut app, "sched_all", "grunt", Vec3::new(0.0, 0.0, 1.2), N);
    let (who, hero) = (id(&app, e), id(&app, hero));
    let err = fails(&mut app, "npc_plan_cooldown", json!({"id": who, "option": "hevy", "seconds": 1.0}));
    assert_eq!(err, format!("{who}'s plan `sched_all` has no option `hevy` (did you mean `heavy`?); options: light, heavy, turn, pair"));
    let err = fails(&mut app, "npc_plan_cooldown", json!({"id": who, "option": "heavy", "seconds": -1.0}));
    assert_eq!(err, "seconds must be >= 0 and finite (is -1)");
    let err = fails(&mut app, "npc_plan_cooldown", json!({"id": who, "option": "heavy", "seconds": 1.0, "mode": "most"}));
    assert_eq!(err, "mode must be `exactly` or `at_least` (is `most`)");
    let not_npc = format!("{hero} is not an NPC (it has no Npc component)");
    assert_eq!(fails(&mut app, "npc_plan_reset", json!({"id": hero})), not_npc);
    assert_eq!(fails(&mut app, "npc_plan_cooldown", json!({"id": hero, "option": "heavy", "seconds": 1.0})), not_npc);
    app.world_mut().get_mut::<Npc>(e).unwrap().plan = None;
    let err = fails(&mut app, "npc_plan_reset", json!({"id": who}));
    assert_eq!(err, format!("{who} has no attack plan; give it one (act set id={who} component=Npc path=plan value=\"...\")"));
    // On a client the host owns the plans: the tools and the calls refuse.
    app.world_mut().get_mut::<Npc>(e).unwrap().plan = Some("sched_all".into());
    app.insert_resource(shards_core::NetRole::Client);
    assert_eq!(fails(&mut app, "npc_plan_reset", json!({"id": who})), HOST_OWNS_PLANS);
    assert_eq!(fails(&mut app, "npc_plan_cooldown", json!({"id": who, "option": "heavy", "seconds": 1.0})), HOST_OWNS_PLANS);
    assert_eq!(set_plan_cooldown(app.world_mut(), e, "heavy", 1.0, CooldownSet::AtLeast), Err(HOST_OWNS_PLANS.to_owned()));
    assert!(plan_of(&app, e).cooldowns.is_empty());
}

// --- 12.2 run 9: the invariant -------------------------------------------------------------------

#[test]
fn the_sequence_rule_judges_the_press_not_the_start() {
    let mut app = app_seeded(9, record_starts);
    let e = duel(&mut app, "sched_free", "grunt");
    // Run until the NPC has pressed its second swing and it has not started: swap to a plan whose
    // rules would refuse it (`light` at most once in a row, one just before).
    let mut swapped = false;
    for _ in 0..600 {
        step(&mut app, 1);
        let plan = plan_of(&app, e);
        let pressed = app.world().get::<AttackToken>(e).unwrap().held.as_ref().is_some_and(|g| g.pressed.is_some() && !g.started);
        if pressed && !plan.history.is_empty() && !swapped {
            app.world_mut().get_mut::<Npc>(e).unwrap().plan = Some("sched_strict".into());
            swapped = true;
        }
    }
    assert!(swapped, "a second swing was pressed");
    // `step` fails the test on a broken rule; the strict plan is judged by its own choices from here.
    assert!(actions(&app, e).len() >= 2, "the pressed swing started under the new plan: {:?}", actions(&app, e));
    let list = ok(&mut app, "assert_list", json!({}));
    assert!(text(&list).contains("npc.plan_sequence  engine rule  armed"), "{}", text(&list));
}

// --- 12.1: the reproduced draw and the report against the planner ------------------------------

/// Both attacks of the fighter on a long action cooldown: it holds a grant and a choice but cannot
/// press, so the planner draws again every tick.
fn held_back(app: &mut App, e: Entity) {
    let cooldowns = Cooldowns(vec![
        Cooldown { action: "jab".into(), left: 2.0 },
        Cooldown { action: "smash".into(), left: 2.0 },
    ]);
    app.world_mut().entity_mut(e).insert(cooldowns);
}

#[test]
fn a_dev_caller_gets_the_draw_the_planner_makes_next() {
    for (plan, least) in [("sched_draw", 1000), ("sched_decide", 50)] {
        draw_matches(plan, least);
    }
}

fn draw_matches(plan: &str, least: usize) {
    let mut matched = 0;
    for seed in 0..50u64 {
        let mut app = app_seeded(seed, record_starts);
        let e = duel(&mut app, plan, "grunt");
        held_back(&mut app, e);
        let id = id(&app, e);
        step(&mut app, 3);
        for _ in 0..30 {
            let report = schedule(&mut app, json!({"id": id}));
            let pick = report["pick"].clone();
            let player = ai_player_report(&mut app, &id);
            step(&mut app, 1);
            if pick.is_null() {
                continue;
            }
            assert!(player["pick"].is_null(), "a Player caller gets no forecast: {player}");
            assert_eq!(pick["if_unchanged"], true);
            assert_eq!(pick["on_tick"], app.world().resource::<ai_core::SimTick>().0);
            assert_eq!(grant_attack(&app, e).as_deref(), pick["option"].as_str(), "seed {seed}: {report}");
            matched += 1;
        }
    }
    assert!(matched >= least, "{plan}: {matched} draws compared");
}

/// `npc_schedule` as a Player-scope agent.
fn ai_player_report(app: &mut App, id: &str) -> Value {
    app.world_mut().resource_mut::<AgentSessions>().configure("fair", Scope::Player, None);
    let out = ai_core::call_tool(app.world_mut(), "fair", "npc_schedule", json!({"id": id}));
    assert!(out.ok, "{}", out.result);
    out.result
}

#[test]
fn off_a_decision_tick_there_is_no_forecast() {
    // `sched_angle` presses on its decision ticks (every nine): between two there is no pick.
    let mut app = app_seeded(1, record_starts);
    let e = slow_turner(&mut app, "sched_angle", 150.0);
    held_back(&mut app, e);
    step(&mut app, 1);
    let id = id(&app, e);
    let (mut with, mut without, mut between) = (0, 0, 0);
    for _ in 0..40 {
        let report = schedule(&mut app, json!({"id": id}));
        if report["pick"].is_null() {
            let says = report["chooses"].as_str().unwrap();
            if says.starts_with("between decisions") {
                between += 1;
            }
            assert!(says.starts_with("between decisions") || says.starts_with("no grant") || says.starts_with("resting") || says.starts_with("running") || says.starts_with("holds"), "{report}");
            without += 1;
        } else {
            with += 1;
        }
        step(&mut app, 1);
    }
    assert!(with >= 1 && without >= 1, "{with} with a pick, {without} without");
    assert!(between >= 1, "the report never said it was between decisions");
}

#[test]
fn what_the_planner_chooses_is_what_the_report_called_eligible() {
    let mut checked = 0;
    for seed in 0..12u64 {
        let mut app = app_seeded(seed, record_starts);
        let world = app.world_mut();
        let hero = player(world, 0, Vec3::ZERO);
        let _ = hero;
        if seed % 2 == 0 {
            player(world, 1, Vec3::new(1.0, 0.0, 0.0));
        }
        let mut npcs = Vec::new();
        for k in 0..3u32 {
            let at = Vec3::new(2.0 * k as f32 - 2.0, 0.0, 1.4 + 0.2 * k as f32);
            let facing = Quat::from_rotation_y((seed * 53 + u64::from(k) * 97) as f32 % 360.0 * std::f32::consts::PI / 180.0) * N;
            let e = npc(world, "sched", Npc::new("grunt", "grunt").with_plan("sched_all").with_director("loose"), at, facing);
            world.entity_mut(e).insert(CharacterParams { turn_speed_deg: 90.0, locked_turn_speed_deg: 90.0, ..default() });
            // Histories and cooldowns that differ by seed, so the bans and the gates have something to judge.
            let mut plan = NpcPlan::default();
            if (seed + u64::from(k)) % 3 == 0 {
                plan.history.push(shards_npc::PastAttack { name: "pair".into(), tags: vec!["zone_center".into()] });
            }
            if (seed + u64::from(k)) % 2 == 0 {
                plan.cooldowns.push(shards_npc::PlanCooldown { name: "turn".into(), left: 0.5 * ((seed + u64::from(k)) % 5) as f32 });
            }
            world.entity_mut(e).insert(plan);
            npcs.push(e);
        }
        for _ in 0..500 {
            let tick = app.world().resource::<ai_core::SimTick>().0;
            let before: Vec<Option<Value>> = npcs
                .iter()
                .map(|e| {
                    let who = id(&app, *e);
                    let out = call(&mut app, "npc_schedule", json!({"id": who}));
                    out.ok.then_some(out.result)
                })
                .collect();
            step(&mut app, 1);
            for (e, report) in npcs.iter().zip(&before) {
                let Some(report) = report else { continue };
                let g = app.world().get::<AttackToken>(*e).unwrap().held.clone();
                let Some(g) = g.filter(|g| g.pressed == Some(tick + 1)) else { continue };
                let name = g.attack.unwrap();
                assert!(eligible(report).contains(&name), "seed {seed}, tick {}: chose {name}, the report said {report}", tick + 1);
                checked += 1;
            }
        }
    }
    assert!(checked >= 100, "{checked} choices compared");
}

#[test]
fn the_players_gate_follows_the_living_players() {
    let mut app = app_seeded(2, record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let e = npc(world, "sched", Npc::new("grunt", "grunt").with_plan("sched_all").with_director("loose"), Vec3::new(0.0, 0.0, 1.2), N);
    step(&mut app, 2);
    let id = id(&app, e);
    let solo = schedule(&mut app, json!({"id": id}));
    assert_eq!(solo["players"], 1);
    assert!(option(&solo, "pair")["refused"].as_str().unwrap().starts_with("players: 1 living, this option needs 2-3"), "{solo}");
    let duo = schedule(&mut app, json!({"id": id, "players": 2}));
    assert!(option(&duo, "pair")["eligible"] == true, "{duo}");
    assert_eq!(duo["what_if"], true);
    let crowd = schedule(&mut app, json!({"id": id, "players": 4}));
    assert!(option(&crowd, "pair")["refused"].as_str().unwrap().starts_with("players: 4 living"), "{crowd}");
    // A second body joins: the count is the world's.
    player(app.world_mut(), 1, Vec3::new(2.0, 0.0, 0.0));
    let two = schedule(&mut app, json!({"id": id}));
    assert_eq!(two["players"], 2);
    assert!(option(&two, "pair")["eligible"] == true, "{two}");
}

#[test]
fn the_tool_names_its_errors_and_says_what_the_npc_is_doing() {
    let mut app = app_seeded(2, record_starts);
    let (hero, e) = arena(&mut app, "sched_all", "grunt", Vec3::new(0.0, 0.0, 1.2), N);
    let id = id(&app, e);
    let hero_id = self::id(&app, hero);
    assert!(fails(&mut app, "npc_schedule", json!({"id": hero_id})).contains("is not an NPC (it has no Npc component)"));
    assert!(fails(&mut app, "npc_schedule", json!({"id": id, "angle": 200})).contains("angle must be 0..180 degrees (is 200)"));
    assert!(fails(&mut app, "npc_schedule", json!({"id": id, "players": 0})).contains("players must be at least 1 (is 0)"));
    // Nobody perceives anybody before the first tick.
    assert!(fails(&mut app, "npc_schedule", json!({"id": id})).contains("has no target: pass target="), "no target yet");
    let judged = schedule(&mut app, json!({"id": id, "target": hero_id}));
    assert_eq!(judged["what_if"], true);
    step(&mut app, 3);
    let report = schedule(&mut app, json!({"id": id}));
    let chooses = report["chooses"].as_str().unwrap();
    assert!(chooses.starts_with("no grant") || chooses.starts_with("the next tick decides") || chooses.starts_with("running") || chooses.starts_with("resting") || chooses.starts_with("holds"), "{chooses}");
    // Without a plan.
    app.world_mut().get_mut::<Npc>(e).unwrap().plan = None;
    assert!(fails(&mut app, "npc_schedule", json!({"id": id, "target": hero_id})).contains("has no attack plan"));
}

#[test]
fn npc_state_counts_the_repeats_of_a_plan_that_limits_them() {
    let mut app = app_seeded(11, record_starts);
    let e = duel(&mut app, "sched_repeats", "grunt");
    for _ in 0..300 {
        step(&mut app, 1);
        if !plan_of(&app, e).history.is_empty() {
            break;
        }
    }
    let who = id(&app, e);
    let state = ok(&mut app, "npc_state", json!({"id": who})).result;
    let name = state["plan"]["history"][0].as_str().unwrap().to_owned();
    assert_eq!(state["plan"]["repeats"], json!({"name": name, "times": 1}), "{state}");
    // A plan without `max_repeats` keeps the old answer.
    let mut other = app_seeded(11, record_starts);
    let e = duel(&mut other, "sched_free", "grunt");
    for _ in 0..300 {
        step(&mut other, 1);
        if !plan_of(&other, e).history.is_empty() {
            break;
        }
    }
    let who = id(&other, e);
    let state = ok(&mut other, "npc_state", json!({"id": who})).result;
    assert!(state["plan"].get("repeats").is_none(), "{state}");
}

// --- 12.2 run 13: content reads --------------------------------------------------------------------

/// The definitions the session read, as the replay's `content` check names them.
fn content_read(app: &App) -> Vec<String> {
    let recording = replay::save(app.world());
    let log = recording.check_parts.get("content").expect("a content check with parts");
    parts_at(log, recording.end.tick).into_keys().collect()
}

#[test]
fn the_queries_read_no_definition_the_simulation_does_not() {
    // 0: no calls; 1: the queries every 30 ticks; 2: the dev tools on the plan's memory too (N8c), one
    // with a misspelt option. The third session plays differently; only what it read is compared.
    let play = |calls: u8| {
        let mut app = app_seeded(4, record_starts);
        let (hero, e) = arena(&mut app, "sched_all", "grunt", Vec3::new(0.0, 0.0, 1.2), N);
        step(&mut app, 1);
        let (id, hero) = (id(&app, e), id(&app, hero));
        for _ in 0..20 {
            if calls >= 1 {
                ok(&mut app, "npc_schedule", json!({"id": id, "target": hero}));
                ok(&mut app, "npc_state", json!({"id": id}));
                ai_player_report(&mut app, &id);
            }
            if calls >= 2 {
                ok(&mut app, "npc_plan_cooldown", json!({"id": id, "option": "heavy", "seconds": 0.5, "mode": "at_least"}));
                fails(&mut app, "npc_plan_cooldown", json!({"id": id, "option": "hevy", "seconds": 0.5}));
                ok(&mut app, "npc_plan_reset", json!({"id": id}));
            }
            step(&mut app, 30);
        }
        content_read(&app)
    };
    let (quiet, asking, resetting) = (play(0), play(1), play(2));
    assert!(quiet.iter().any(|p| p.starts_with("attack_plan/")), "{quiet:?}");
    assert_eq!(quiet, asking);
    assert_eq!(quiet, resetting);
}

// --- N8b: tokenless grants and a plan's waiting role ----------------------------------------------

/// A hero at the origin and one NPC per plan around it (south, east, west), 1.2 m out and facing it,
/// all fighting by `director`.
fn ring(app: &mut App, director: &str, plans: &[&str]) -> Vec<Entity> {
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let spots = [Vec3::new(0.0, 0.0, 1.2), Vec3::new(1.2, 0.0, 0.0), Vec3::new(-1.2, 0.0, 0.0)];
    plans
        .iter()
        .zip(spots)
        .map(|(plan, at)| npc(world, "sched", Npc::new("grunt", "grunt").with_plan(*plan).with_director(director), at, (-at).normalize()))
        .collect()
}

fn jabs(app: &App, who: Entity) -> Vec<u64> {
    app.world().resource::<Started>().0.iter().filter(|(_, e, a)| *e == who && a == "jab").map(|(t, ..)| *t).collect()
}

fn grant(app: &App, who: Entity) -> Option<Grant> {
    app.world().get::<AttackToken>(who).unwrap().held.clone()
}

fn flying(app: &App, who: Entity) -> bool {
    grant(app, who).is_some_and(|g| g.in_flight())
}

/// Whether a journal line (`tick text`) says `who` (`e7`) does something: `12 e7 takes an attack token ...`.
fn says(line: &str, who: &str) -> bool {
    line.split_whitespace().nth(1) == Some(who)
}

// --- 12.2 run 3: a boss among its adds ---------------------------------------------------------

/// One tokenless boss with `boss` as its plan and two adds on one director that spaces strikes 0.7 s
/// (42 ticks) and has two tokens, fighting for `ticks` ticks.
fn boss_among_adds(boss: &str, ticks: u64) -> (App, Vec<Entity>) {
    let mut app = app_seeded(7, record_starts);
    let who = ring(&mut app, "sched_arena", &[boss, "sched_add", "sched_add"]);
    // `step` fails the test when `npc.tokens_within_limit`, `npc.strike_spacing` or `npc.plan_sequence` breaks.
    step(&mut app, ticks);
    (app, who)
}

#[test]
fn a_boss_takes_no_token_and_delays_the_adds_that_never_delay_it() {
    let (app, who) = boss_among_adds("sched_boss", 2400);
    let (boss, adds) = (jabs(&app, who[0]), [jabs(&app, who[1]), jabs(&app, who[2])]);
    assert!(boss.len() >= 8 && adds.iter().all(|a| a.len() >= 3), "everyone strikes: boss {boss:?}, adds {adds:?}");
    // (a) An add never starts a strike within 42 ticks after the boss's start.
    for add in &adds {
        for t in add {
            if let Some(b) = boss.iter().rfind(|b| **b <= *t) {
                assert!(t - b >= 42, "an add began at {t}, {} ticks after the boss at {b}", t - b);
            }
        }
    }
    // (b) The boss starts within 42 ticks after an add's start at least once: it does not wait.
    let early = boss.iter().any(|b| adds.iter().flatten().any(|a| *a < *b && b - a < 42));
    assert!(early, "the boss never struck inside an add's spacing: boss {boss:?}, adds {adds:?}");
    // (c) The adds take the director's tokens, the boss none: `(1/2)` and `(2/2)` name the adds only.
    let boss_id = id(&app, who[0]);
    let granted = lines(&app, "takes an attack token");
    assert!(granted.len() >= 6, "{granted:?}");
    assert!(granted.iter().all(|l| !says(l, &boss_id)), "{granted:?}");
    assert!(granted.iter().any(|l| l.ends_with("(1/2)")), "{granted:?}");
    assert!(lines(&app, "returns its attack token").iter().all(|l| !says(l, &boss_id)));
    // (d) The rules held on every tick (`step` would have stopped): the books and the spacing.
    assert_eq!(shards_npc::tokens_within_limit(app.world()), None);
    assert_eq!(shards_npc::strike_spacing(app.world()), None);
}

#[test]
fn both_adds_hold_a_token_while_the_boss_attacks() {
    let mut app = app_seeded(7, record_starts);
    let who = ring(&mut app, "sched_arena", &["sched_boss", "sched_add", "sched_add"]);
    let mut together = 0;
    for _ in 0..2400 {
        step(&mut app, 1);
        if flying(&app, who[0]) && has_token(&app, who[1]) && has_token(&app, who[2]) {
            together += 1;
        }
    }
    assert!(together >= 1, "the boss's attack never overlapped two token holders");
    // Its grant is not one of the tokens: both of the adds' two were out.
    assert!(!lines(&app, "(2/2)").is_empty());
}

#[test]
fn without_the_mode_the_boss_takes_a_token_like_anyone() {
    let (app, who) = boss_among_adds("sched_boss_counted", 1200);
    let boss_id = id(&app, who[0]);
    assert!(lines(&app, "takes an attack token").iter().any(|l| says(l, &boss_id)), "the counted boss takes tokens");
}

// --- 12.2 run 4: the books of a tokenless grant -----------------------------------------------------

#[test]
fn a_tokenless_grant_with_nothing_in_reach_is_never_returned_unused() {
    let mut app = app_seeded(3, record_starts);
    let who = ring(&mut app, "sched_arena", &["sched_stuck_boss"]);
    step(&mut app, 120);
    let held = grant(&app, who[0]).expect("a grant: an option may be chosen, distance aside");
    step(&mut app, 600);
    let later = grant(&app, who[0]).expect("still holds it");
    assert_eq!(later.since, held.since, "never returned and granted again");
    assert!(later.age > 10.0, "held for {} s with patience 3", later.age);
    assert!(lines(&app, "attack token").is_empty(), "no token event: {:?}", lines(&app, "attack token"));
    // The control: by the tokens, the same plan gives it back after `patience`.
    let mut counted = app_seeded(3, record_starts);
    ring(&mut counted, "sched_arena", &["sched_stuck"]);
    step(&mut counted, 720);
    assert!(lines(&counted, "returns its attack token unused: unused").len() >= 2, "{:?}", lines(&counted, "attack token"));
}

/// A hero, a far hero (a second living player, out of everyone's sight), a counted add and the boss
/// (spawned after it) on director `sched_lowering` (one token for one player, two for two), both
/// with nothing in reach so their grants stay.
fn two_players(boss: &str) -> (App, Entity, Entity, Entity) {
    let mut app = app_seeded(3, record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let far = player(world, 1, Vec3::new(55.0, 0.0, 55.0));
    let mk = |world: &mut World, plan: &str, at: Vec3| {
        npc(world, "sched", Npc::new("grunt", "grunt").with_plan(plan).with_director("sched_lowering"), at, (-at).normalize())
    };
    let add = mk(world, "sched_stuck", Vec3::new(1.2, 0.0, 0.0));
    let boss = mk(world, boss, Vec3::new(0.0, 0.0, 1.2));
    (app, far, add, boss)
}

#[test]
fn a_lowered_limit_never_takes_a_tokenless_grant() {
    let (mut app, far, add, boss) = two_players("sched_stuck_boss");
    step(&mut app, 120);
    assert!(has_token(&app, add) && has_token(&app, boss), "both hold a grant");
    let held = grant(&app, boss).unwrap().since;
    // A player dies: one token is left, and it is the add's.
    app.world_mut().entity_mut(far).insert(Dead);
    step(&mut app, 60);
    assert!(has_token(&app, add) && has_token(&app, boss), "the add's counted token fits, the boss's is outside the books");
    assert_eq!(grant(&app, boss).unwrap().since, held);
    assert!(lines(&app, "limit lowered").is_empty(), "{:?}", lines(&app, "attack token"));
    // The control: counted, the boss's grant is the newest and goes back.
    let (mut counted, far, add, boss) = two_players("sched_stuck");
    step(&mut counted, 120);
    counted.world_mut().entity_mut(far).insert(Dead);
    step(&mut counted, 60);
    assert!(has_token(&counted, add) && !has_token(&counted, boss), "the newest grant goes back");
    assert_eq!(lines(&counted, "limit lowered").len(), 1, "{:?}", lines(&counted, "attack token"));
}

/// The far boss's plan as content text, with `tokenless` on or off.
fn far_boss(tokenless: bool) -> Value {
    json!({
        "options": [{"input": "light", "min": 8.0, "max": 9.0}],
        "in_range_only": true,
        "rest": [0.1, 0.1],
        "first_rest": [1.0, 1.0],
        "tokenless": tokenless,
        "wait": {"role": "Strafe", "distance": 2.4, "speed": 1.0, "orbit": 2.0, "period": 1.0},
    })
}

#[test]
fn a_hot_reload_that_turns_tokenless_off_counts_the_grant_and_gives_it_back_over_the_limit() {
    let mut app = app_seeded(3, record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let mk = |world: &mut World, plan: &str, at: Vec3| {
        npc(world, "sched", Npc::new("grunt", "grunt").with_plan(plan).with_director("sched_lowering"), at, (-at).normalize())
    };
    let add = mk(world, "sched_stuck", Vec3::new(1.2, 0.0, 0.0));
    let boss = mk(world, "sched_stuck_boss", Vec3::new(0.0, 0.0, 1.2));
    step(&mut app, 120);
    assert!(has_token(&app, add) && has_token(&app, boss), "one counted token (1/1) and the boss's grant");
    assert!(lines(&app, "(1/1)").len() == 1, "{:?}", lines(&app, "attack token"));
    let change = json!([{"kind": "attack_plan", "id": "sched_stuck_boss", "before": far_boss(true), "def": far_boss(false)}]);
    ok(&mut app, "content_apply", json!({"changes": change}));
    step(&mut app, 2);
    // Counted now, it is over the limit and not in flight: the newest grant goes back, and it does not get one again.
    assert!(has_token(&app, add) && !has_token(&app, boss));
    assert_eq!(lines(&app, "limit lowered").len(), 1, "{:?}", lines(&app, "attack token"));
    step(&mut app, 20);
    assert!(!has_token(&app, boss), "the one token is the add's");
    // And back: tokenless again, it takes a grant at once, outside the limit.
    let change = json!([{"kind": "attack_plan", "id": "sched_stuck_boss", "before": far_boss(false), "def": far_boss(true)}]);
    ok(&mut app, "content_apply", json!({"changes": change}));
    step(&mut app, 10);
    assert!(has_token(&app, boss), "tokenless again, it holds a grant the limit does not count");
}

// --- 12.2 run 5: the plan's own waiting role ---------------------------------------------------------

/// The change of `orbit` between two ticks, wrapped to -pi..pi.
fn turn(a: f32, b: f32) -> f32 {
    (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

#[test]
fn an_npc_with_its_own_waiting_role_takes_no_slot_and_moves_by_its_role() {
    let mut app = app_seeded(3, record_starts);
    // The adds cannot be granted (their only option is on a long cooldown), so they wait in slots.
    let who = ring(&mut app, "sched_slots", &["sched_stuck_boss", "sched_cooled", "sched_cooled"]);
    for add in &who[1..] {
        app.world_mut().get_mut::<NpcPlan>(*add).unwrap().cooldowns.push(shards_npc::PlanCooldown { name: "light".into(), left: 99.0 });
    }
    let mut samples: Vec<(bool, Option<String>, Option<u32>, f32)> = Vec::new();
    for _ in 0..240 {
        step(&mut app, 1);
        let brain = app.world().get::<NpcBrain>(who[0]).unwrap();
        samples.push((has_token(&app, who[0]), brain.role.clone(), brain.slot, brain.orbit));
    }
    // The adds took slots 0 and 1; the boss none.
    let mut slots: Vec<u32> = who[1..].iter().filter_map(|e| app.world().get::<NpcBrain>(*e).unwrap().slot).collect();
    slots.sort_unstable();
    assert_eq!(slots, [0, 1], "the others' slots do not move");
    assert!(samples.iter().all(|s| s.2.is_none()), "the boss never takes a slot");
    // Without a grant (its first rest, 1 s) and with one and nothing in reach, it moves by its role,
    // its orbit changing from tick to tick.
    let moving = |range: std::ops::Range<usize>, held: bool| {
        let part: Vec<_> = samples[range].iter().filter(|s| s.0 == held && s.1.as_deref() == Some("strafe")).collect();
        let changes = part.windows(2).filter(|w| turn(w[0].3, w[1].3).abs() > 1e-5).count();
        (part.len(), changes)
    };
    let (len, changes) = moving(5..55, false);
    assert!(len >= 40 && changes + 2 >= len, "waiting without a grant: {changes} changes in {len} ticks");
    let (len, changes) = moving(90..240, true);
    assert!(len >= 120 && changes + 2 >= len, "holding a grant with nothing in reach: {changes} changes in {len} ticks");
    // The strafe turns back after its period (1 s = 60 ticks): both directions in 150 ticks.
    let turns: Vec<f32> = samples[90..240].windows(2).map(|w| turn(w[0].3, w[1].3)).collect();
    assert!(turns.iter().any(|t| *t > 1e-5) && turns.iter().any(|t| *t < -1e-5), "one direction only");
    let flips = turns.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
    assert!((1..=4).contains(&flips), "{flips} turns in 2.5 s with a period of 1 s");
}

#[test]
fn a_press_clears_the_role_until_it_waits_again() {
    let mut app = app_seeded(3, record_starts);
    let who = ring(&mut app, "sched_arena", &["sched_boss"]);
    let boss = who[0];
    let (mut pressed, mut after_press, mut waited_again) = (0, Vec::new(), 0);
    for _ in 0..900 {
        step(&mut app, 1);
        let now = app.world().resource::<ai_core::SimTick>().0;
        let role = app.world().get::<NpcBrain>(boss).unwrap().role.clone();
        match grant(&app, boss) {
            Some(g) if g.pressed == Some(now) => {
                pressed += 1;
                assert_eq!(role, None, "tick {now}: the press clears the role");
            }
            Some(g) if g.in_flight() => after_press.push(role),
            _ if role.is_some() && !after_press.is_empty() => waited_again += 1,
            _ => {}
        }
    }
    assert!(pressed >= 5, "{pressed} presses");
    assert!(after_press.iter().all(Option::is_none), "no role while the attack is in flight");
    assert!(waited_again >= 5, "and it waits in its role again after each: {waited_again}");
}

// --- 12.2 run 11: the budget ------------------------------------------------------------------------

#[test]
fn a_boss_presses_while_an_adds_attack_is_in_flight_and_the_add_waits_for_the_boss() {
    let run = |boss: &str| {
        let mut app = app_seeded(5, record_starts);
        let who = ring(&mut app, "sched_budget", &[boss, "sched_add"]);
        step(&mut app, 1800);
        (jabs(&app, who[0]), jabs(&app, who[1]))
    };
    // A jab is 54 ticks long. The boss costs 1.0 and the target's budget is 1.0.
    let (boss, add) = run("sched_boss_quick");
    assert!(boss.len() >= 8 && add.len() >= 3, "boss {boss:?}, add {add:?}");
    assert!(
        boss.iter().any(|b| add.iter().any(|a| *b > *a + 2 && *b < *a + 50)),
        "the boss never struck inside the add's attack: boss {boss:?}, add {add:?}"
    );
    assert!(
        add.iter().all(|a| boss.iter().all(|b| *a <= *b + 2 || *a >= *b + 50)),
        "an add struck inside the boss's attack: boss {boss:?}, add {add:?}"
    );
    // By the tokens (the control), the boss waits for the add like anyone.
    let (boss, add) = run("sched_boss_quick_counted");
    assert!(boss.len() >= 3 && add.len() >= 3, "boss {boss:?}, add {add:?}");
    assert!(boss.iter().all(|b| add.iter().all(|a| *b <= *a + 2 || *b >= *a + 50)), "a counted boss struck inside the add's attack");
}

// --- 12.2 run 10: snapshots ---------------------------------------------------------------------------

#[test]
fn a_fight_with_a_tokenless_boss_loaded_at_any_tick_ends_the_same() {
    use ai_core::snapshot::{self, differences};
    let end = 420;
    let mut original = app_seeded(7, |_| {});
    let who = ring(&mut original, "sched_arena", &["sched_boss", "sched_add", "sched_add"]);
    let (mut saved, mut held) = (Vec::new(), 0);
    for at in (30..end).step_by(30) {
        step(&mut original, 30);
        held += usize::from(grant(&original, who[0]).is_some());
        saved.push((at, snapshot::capture(original.world()).unwrap()));
    }
    step(&mut original, 30);
    let last = snapshot::capture(original.world()).unwrap();
    assert!(held >= 6, "the boss held a grant at {held} checkpoints");
    let boss = last.entities.iter().find(|e| e.id == aid(&original, who[0]).0).unwrap();
    assert!(boss.components.keys().any(|k| k.ends_with("::AttackToken")));
    for (at, snapshot) in saved {
        let mut copy = app_seeded(7, |_| {});
        let report = snapshot::restore(copy.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("tick {at}: {e}"));
        assert!(report.exact(), "tick {at}: {:?}", report.differences);
        step(&mut copy, end - at);
        let diff = differences(&last, &snapshot::capture(copy.world()).unwrap());
        assert!(diff.is_empty(), "loaded at tick {at}:\n{}", diff.join("\n"));
    }
}


/// Run 5, between decisions: a boss on a plan decided by the NPC's think cadence holds a grant with
/// nothing in reach, and on the ticks that are not decisions its role stays and its orbit moves.
#[test]
fn a_waiting_role_is_kept_and_moves_the_orbit_between_decision_ticks_too() {
    let mut app = app_seeded(3, record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let boss = npc(world, "sched", Npc::new("grunt_think", "grunt").with_plan("sched_stuck_decide").with_director("sched_arena"), Vec3::new(0.0, 0.0, 1.2), N);
    step(&mut app, 100);
    let boss_id = id(&app, boss);
    let (mut between, mut moved) = (0, 0);
    let mut last = app.world().get::<NpcBrain>(boss).unwrap().orbit;
    for _ in 0..150 {
        let says = schedule(&mut app, json!({"id": boss_id}))["chooses"].as_str().unwrap().to_owned();
        step(&mut app, 1);
        let brain = app.world().get::<NpcBrain>(boss).unwrap();
        if says == "between decisions: waits in its role" {
            between += 1;
            assert_eq!(brain.role.as_deref(), Some("strafe"), "the role is kept between decisions");
            assert!(brain.slot.is_none(), "and no slot is taken");
            if turn(last, brain.orbit).abs() > 1e-5 {
                moved += 1;
            }
        }
        last = brain.orbit;
    }
    assert!(between >= 20, "{between} ticks between decisions");
    assert!(moved + 2 >= between, "the orbit moved on {moved} of {between} ticks between decisions");
}

#[test]
fn the_tools_say_when_a_grant_is_not_one_of_the_tokens() {
    let mut app = app_seeded(3, record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let boss = npc(world, "sched", Npc::new("grunt_think", "grunt").with_plan("sched_stuck_decide").with_director("sched_arena"), Vec3::new(0.0, 0.0, 1.2), N);
    let add = npc(world, "sched", Npc::new("grunt_think", "grunt").with_plan("sched_stuck").with_director("sched_arena"), Vec3::new(1.2, 0.0, 0.0), W);
    step(&mut app, 20);
    let (boss_id, add_id) = (id(&app, boss), id(&app, add));
    // Its first rest: no grant yet, and it does not say tokens are in use.
    let early = schedule(&mut app, json!({"id": boss_id}));
    assert_eq!(early["tokenless"], true);
    assert!(early["chooses"].as_str().unwrap().starts_with("resting"), "{early}");
    step(&mut app, 100);
    let state = ok(&mut app, "npc_state", json!({"id": boss_id})).result;
    assert_eq!(state["token"]["counted"], false, "{state}");
    assert_eq!(state["plan"]["tokenless"], true);
    assert_eq!(state["role"], "strafe");
    // Nothing in reach: between its decisions it waits in its role.
    let mut says = Vec::new();
    for _ in 0..20 {
        says.push(schedule(&mut app, json!({"id": boss_id}))["chooses"].as_str().unwrap().to_owned());
        step(&mut app, 1);
    }
    assert!(says.iter().any(|s| s == "between decisions: waits in its role"), "{says:?}");
    assert!(says.iter().any(|s| s == "the next tick decides"), "{says:?}");
    // An NPC that fights by the tokens keeps the old answers.
    let state = ok(&mut app, "npc_state", json!({"id": add_id})).result;
    assert!(state["token"].get("counted").is_none() && state["plan"].get("tokenless").is_none(), "{state}");
    let counted = schedule(&mut app, json!({"id": add_id}));
    assert!(counted.get("tokenless").is_none(), "{counted}");
}

#[test]
fn the_tokens_in_use_leave_out_a_tokenless_grant() {
    let mut app = app_seeded(3, record_starts);
    let world = app.world_mut();
    player(world, 0, Vec3::ZERO);
    let boss = npc(world, "sched", Npc::new("grunt", "grunt").with_plan("sched_stuck_boss").with_director("sched_arena"), Vec3::new(0.0, 0.0, 1.2), N);
    let add = npc(world, "sched", Npc::new("grunt", "grunt").with_plan("sched_cooled").with_director("sched_arena"), Vec3::new(1.2, 0.0, 0.0), W);
    app.world_mut().get_mut::<NpcPlan>(add).unwrap().cooldowns.push(shards_npc::PlanCooldown { name: "light".into(), left: 99.0 });
    step(&mut app, 120);
    assert!(has_token(&app, boss) && !has_token(&app, add));
    // The boss holds a grant; the director has two tokens and none is out, and the add says so.
    let add_id = id(&app, add);
    let report = schedule(&mut app, json!({"id": add_id}));
    assert_eq!(report["chooses"], "no grant yet: tokens 0/2 in use", "{report}");
}
