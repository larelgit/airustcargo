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
use shards_action::{Cooldown, Cooldowns};
use shards_motion::CharacterParams;
use shards_npc::prelude::*;
use shards_npc::NpcPlan;

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
    let play = |calls: bool| {
        let mut app = app_seeded(4, record_starts);
        let (hero, e) = arena(&mut app, "sched_all", "grunt", Vec3::new(0.0, 0.0, 1.2), N);
        step(&mut app, 1);
        let (id, hero) = (id(&app, e), id(&app, hero));
        for _ in 0..20 {
            if calls {
                ok(&mut app, "npc_schedule", json!({"id": id, "target": hero}));
                ok(&mut app, "npc_state", json!({"id": id}));
                ai_player_report(&mut app, &id);
            }
            step(&mut app, 30);
        }
        content_read(&app)
    };
    let (quiet, asking) = (play(false), play(true));
    assert!(quiet.iter().any(|p| p.starts_with("attack_plan/")), "{quiet:?}");
    assert_eq!(quiet, asking);
}
