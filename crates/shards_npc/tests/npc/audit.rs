//! The audit of an attack plan (N8e): which options may follow which, the gap between one's danger
//! and the next one's first hit, every hit a plan can deal, and the `plan_audit` query. Test content
//! in `assets/content/audit.*.ron`, with windows chosen so every gap is counted by hand: `au_quick`
//! 1.0 s (hits 0.3-0.5), `au_slow` 1.2 s (0.6-0.8), `au_combo_1` 0.9 s (0.2-0.3) going on from its
//! recovery (0.3) into `au_combo_2` 0.5 s (0.2-0.3), `au_zone` 1.3 s (0.2-1.2, a pulse every 0.25 s)
//! then `au_zone_tail` 0.3 s (all of it), which names the zone again; the plan rests at least 0.5 s.
//! `au_spawn_plan` times what its attacks spawn and `au_cut_plan` cuts one by a chain (their tests say
//! how).

use serde_json::json;
use shards_content::ContentDb;
use shards_npc::audit::{follow_pairs, may_follow, plan_hits, unreachable_options, FollowPair};
use shards_npc::{resolve, AttackPlanDef};

use crate::common::*;

fn db() -> ContentDb {
    app().world().resource::<ContentDb>().clone()
}

fn pair<'a>(pairs: &'a [FollowPair], a: &str, b: &str) -> Option<&'a FollowPair> {
    pairs.iter().find(|p| p.a == a && p.b == b)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn the_sequence_rules_decide_what_may_follow() {
    let db = db();
    let plan = db.get::<AttackPlanDef>("au_plan").unwrap();
    let options = resolve(plan, &db, "au", 0.0);
    let by = |name: &str| options.iter().find(|o| o.name == name).unwrap();
    // `max_repeats: 1`: never twice in a row; `forbid_after: ["big"]` on an option tagged `big`: the same;
    // the ban: never `quick` right after `zone`.
    assert!(!may_follow(plan, by("quick"), by("quick")));
    assert!(!may_follow(plan, by("slow"), by("slow")));
    assert!(!may_follow(plan, by("zone"), by("quick")));
    // The rest may.
    assert!(may_follow(plan, by("quick"), by("slow")));
    assert!(may_follow(plan, by("zone"), by("zone")));
    assert!(may_follow(plan, by("combo"), by("combo")));
    let pairs = follow_pairs(&db, "au_plan", "au", 1.0).unwrap();
    for (a, b) in [("quick", "quick"), ("slow", "slow"), ("zone", "quick")] {
        assert!(pair(&pairs, a, b).is_none(), "{a} -> {b} is refused: {pairs:#?}");
    }
    assert_eq!(pairs.len(), 4 * 4 - 3, "every other ordered pair of the four reachable options");
}

#[test]
fn the_gaps_are_counted_from_the_windows_and_the_shortest_rest() {
    let pairs = follow_pairs(&db(), "au_plan", "au", 1.0).unwrap();
    // quick then slow: quick's danger ends at 0.5; slow starts at 1.0 + 0.5 and hits from 0.6 after.
    let p = pair(&pairs, "quick", "slow").unwrap();
    assert!(close(p.a_end, 0.5) && close(p.b_first, 2.1) && close(p.gap, 1.6) && !p.chained, "{p:?}");
    // slow then quick: 0.8, then 1.2 + 0.5 + 0.3.
    let p = pair(&pairs, "slow", "quick").unwrap();
    assert!(close(p.a_end, 0.8) && close(p.b_first, 2.0) && close(p.gap, 1.2), "{p:?}");
    // The zone runs through its tail (the loop back is counted once): 1.6 s, danger until 1.6.
    let p = pair(&pairs, "zone", "slow").unwrap();
    assert!(close(p.a_end, 1.6) && close(p.b_first, 2.7) && close(p.gap, 1.1), "{p:?}");
    // Sorted shortest first.
    assert!(pairs.windows(2).all(|w| w[0].gap <= w[1].gap), "{pairs:#?}");
}

#[test]
fn a_chain_keeps_the_smaller_gap_of_its_two_ways() {
    let pairs = follow_pairs(&db(), "au_plan", "au", 1.0).unwrap();
    // Without the chain: combo's danger ends at 0.3 and it ends at 0.9: quick hits from 0.9 + 0.5 + 0.3
    // (gap 1.4). Through it: the continuation starts at 0.3 and hits until 0.6, the chain ends at 0.8:
    // quick hits from 0.8 + 0.5 + 0.3 = 1.6 (gap 1.0). The smaller is kept.
    let p = pair(&pairs, "combo", "quick").unwrap();
    assert!(p.chained && close(p.a_end, 0.6) && close(p.b_first, 1.6) && close(p.gap, 1.0), "{p:?}");
}

#[test]
fn a_rest_scale_shortens_every_gap_by_the_rest_it_takes_away() {
    let db = db();
    let (full, hard) = (follow_pairs(&db, "au_plan", "au", 1.0).unwrap(), follow_pairs(&db, "au_plan", "au", 0.85).unwrap());
    assert_eq!(full.len(), hard.len());
    for p in &full {
        let q = pair(&hard, &p.a, &p.b).unwrap();
        assert!(close(p.gap - q.gap, 0.15 * 0.5), "{p:?} {q:?}");
    }
}

#[test]
fn every_hit_is_found_through_chains_and_then() {
    let db = db();
    let hits = plan_hits(&db, "au_plan", "au").unwrap();
    let of = |option: &str| hits.iter().filter(|h| h.option == option).map(|h| (h.action.as_str(), h.via.clone(), h.times)).collect::<Vec<_>>();
    assert_eq!(of("quick"), [("au_quick", vec![], 1)]);
    assert_eq!(of("combo"), [("au_combo_1", vec![], 1), ("au_combo_2", vec!["au_combo_1".to_owned()], 1)]);
    // The zone's pulse lands at 0.2 and every 0.25 s while its 1.0 s window lasts: 5 times; the loop
    // back from the tail is not followed again.
    assert_eq!(of("zone"), [("au_zone", vec![], 5), ("au_zone_tail", vec!["au_zone".to_owned()], 1)]);
    let slow = hits.iter().find(|h| h.option == "slow").unwrap();
    assert_eq!((slow.damage, slow.poise, slow.flags.len()), (3.0, 5.0, 1));
    assert_eq!(unreachable_options(&db, "au_plan", "au").unwrap(), ["nowhere"]);
}

#[test]
fn plan_audit_answers_and_says_how_to_fix_a_bad_call() {
    let mut app = app();
    let out = ok(&mut app, "plan_audit", json!({"plan": "au_plan", "moveset": "au", "min_gap": 1.1})).result;
    // The closest: the zone's danger lasts its whole 1.6 s, and the combo or the zone again hits 0.2 s
    // after the 0.5 s rest (0.7); by name on a tie.
    let first = &out["pairs"][0];
    assert_eq!((&first["a"], &first["b"], &first["gap"]), (&json!("zone"), &json!("combo"), &json!(0.7)), "{out}");
    let chained = out["pairs"].as_array().unwrap().iter().find(|p| p["a"] == "combo" && p["b"] == "quick").unwrap();
    assert_eq!((&chained["gap"], &chained["chained"]), (&json!(1.0), &json!(true)), "{chained}");
    let short = out["short"].as_array().unwrap();
    assert_eq!(short.len(), 5, "zone-combo, zone-zone 0.7, combo-combo, combo-zone 0.9, combo-quick 1.0: {out}");
    assert_eq!(short[0]["why"], "zone's danger lasts until 1.60 s; combo hits from 2.30 s");
    assert_eq!(short[4]["why"], "combo's danger lasts until 0.60 s (through its chain); quick hits from 1.60 s");
    assert_eq!(out["unreachable"], json!(["nowhere"]));
    assert!(out["hits"].as_array().unwrap().iter().any(|h| h["times"] == 5));
    let without = ok(&mut app, "plan_audit", json!({"plan": "au_plan", "moveset": "au"})).result;
    assert!(without.get("short").is_none(), "no threshold of the engine's own: {without}");
    for (args, want) in [
        (json!({"plan": "au_plam", "moveset": "au"}), "no attack_plan `au_plam` (did you mean `au_plan`?)"),
        (json!({"plan": "au_plan", "moveset": "ua"}), "no moveset `ua`"),
        (json!({"plan": "au_plan", "moveset": "au", "min_gap": -1.0}), "min_gap must be >= 0 and finite (is -1)"),
        (json!({"plan": "au_plan", "moveset": "au", "rest_scale": 0.0}), "rest_scale must be > 0 and finite (is 0)"),
        (json!({"plan": "au_plan"}), "plan_audit needs moveset=<id>; no prefab's NPC names this plan"),
    ] {
        let err = fails(&mut app, "plan_audit", args.clone());
        assert!(err.contains(want), "{args}: {err}");
    }
}

/// `au_spawn_plan` (moveset `au_spawn`): `au_cast` 1.0 s lays three bolts at 0.4, 0.5 and 0.6 that fly
/// 1.5 s (their `lifetime`; the flight action is 2.0 s) and burst for 0.1-0.3 s where they stop;
/// `au_slam` 1.0 s hits 0.5-0.6 and lays four pools at 0.6 that burn 2.0 s; `au_call` lays a pool that
/// stays (a summon).
#[test]
fn what_an_attack_spawns_is_part_of_its_danger() {
    let pairs = follow_pairs(&db(), "au_spawn_plan", "au_spawn", 1.0).unwrap();
    // The last bolt flies from 0.6 to 2.1 and may burst until 2.4; the first one may hit at 0.4 (a bolt
    // may stop where it starts).
    let p = pair(&pairs, "cast", "quick").unwrap();
    assert!(close(p.a_end, 2.4) && close(p.b_first, 1.8) && close(p.gap, -0.6), "{p:?}");
    let p = pair(&pairs, "quick", "cast").unwrap();
    assert!(close(p.b_first, 1.9) && close(p.gap, 1.4), "{p:?}");
    // The pools burn until 0.6 + 2.0, long after the slam itself.
    let p = pair(&pairs, "slam", "quick").unwrap();
    assert!(close(p.a_end, 2.6) && close(p.gap, -0.8), "{p:?}");
    assert_eq!((pairs[0].a.as_str(), pairs[0].b.as_str()), ("slam", "quick"), "{pairs:#?}");
    // The summon fights on its own: `call` deals nothing the audit times.
    assert!(pairs.iter().all(|p| p.a != "call" && p.b != "call"), "{pairs:#?}");
    assert_eq!(pairs.len(), 3 * 3);
}

#[test]
fn the_hits_of_what_an_attack_spawns_are_counted_by_object() {
    let hits = plan_hits(&db(), "au_spawn_plan", "au_spawn").unwrap();
    let all: Vec<_> = hits.iter().map(|h| (h.option.as_str(), h.action.as_str(), h.via.join(" > "), h.times)).collect();
    let via = |s: &str| s.to_owned();
    assert_eq!(
        all,
        [
            ("cast", "au_bolt_fly", via("au_cast"), 3),
            ("cast", "au_burst", via("au_cast > au_bolt_fly"), 3),
            ("slam", "au_slam", via(""), 1),
            // Two lines of two pools, each pulsing at 0, 0.5 ... 2.0.
            ("slam", "au_burning", via("au_slam"), 4 * 5),
            ("quick", "au_quick", via(""), 1),
        ]
    );
}

/// `au_cut_plan` (moveset `au_cut`): `au_lob` 2.0 s hits 0.2-0.6, lays a pool at 0.2 and another at 1.0
/// (each burns 2.0 s), and goes on from its recovery (0.6) into `au_lob_2` 0.5 s (0.2-0.3); `jab` is
/// `au_quick` (0.3-0.5); `idle` has weight 0.
#[test]
fn a_chain_cuts_its_attack_but_not_what_the_attack_laid_down() {
    let pairs = follow_pairs(&db(), "au_cut_plan", "au_cut", 1.0).unwrap();
    // Without the chain: the second pool burns until 3.0; jab hits from 2.0 + 0.5 + 0.3 (gap -0.2).
    // Through it: the cut at 0.6 ends the lob's own window and the second pool is never laid, but the
    // first burns on until 2.2; the chain ends at 1.1, so jab hits from 1.9 (gap -0.3).
    let p = pair(&pairs, "lob", "jab").unwrap();
    assert!(p.chained && close(p.a_end, 2.2) && close(p.b_first, 1.9) && close(p.gap, -0.3), "{p:?}");
    let p = pair(&pairs, "jab", "lob").unwrap();
    assert!(close(p.a_end, 0.5) && close(p.b_first, 1.7) && close(p.gap, 1.2), "{p:?}");
    // `idle` is never chosen: no pair and no hit of it.
    assert!(pairs.iter().all(|p| p.a != "idle" && p.b != "idle"), "{pairs:#?}");
    assert_eq!(pairs.len(), 2 * 2);
    let hits = plan_hits(&db(), "au_cut_plan", "au_cut").unwrap();
    let mut options: Vec<&str> = hits.iter().map(|h| h.option.as_str()).collect();
    options.dedup();
    assert_eq!(options, ["lob", "jab"], "{hits:#?}");
}

#[test]
fn plan_audit_finds_a_plan_written_without_some() {
    let mut app = app();
    let err = fails(&mut app, "plan_audit", json!({"plan": "au_cut_plan"}));
    assert!(err.contains("plan_audit needs moveset=<id>; the plan's NPCs use: au_cut"), "{err}");
}
